//! On-disk scrollback for session restore: a pane's lines, saved so a
//! later process can seed a fresh terminal with them.
//!
//! Images are dropped (their pixels are not part of a Line's serde form
//! anyway) and hyperlinks are re-shared on load, so a run of cells that
//! pointed at one `Arc<Hyperlink>` does so again.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use wezterm_term::{Hyperlink, Line};

/// Bumped when the encoding changes; mismatches are refused rather than
/// guessed at.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedScrollback {
    version: u32,
    /// Width the lines were laid out for; the loader re-wraps when the
    /// new pane is a different width.
    pub cols: usize,
    pub lines: Vec<Line>,
}

impl SavedScrollback {
    pub fn new(cols: usize, mut lines: Vec<Line>) -> Self {
        for line in &mut lines {
            for cell in line.cells_mut_for_attr_changes_only() {
                cell.attrs_mut().clear_images();
            }
        }
        Self {
            version: VERSION,
            cols,
            lines,
        }
    }

    /// varbincode inside zstd: the same framing the mux protocol uses.
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        let mut raw = vec![];
        let mut encoder = varbincode::Serializer::new(&mut raw);
        self.serialize(&mut encoder)
            .context("serialize scrollback")?;
        zstd::stream::encode_all(raw.as_slice(), zstd::DEFAULT_COMPRESSION_LEVEL)
            .context("compress scrollback")
    }

    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let raw = zstd::stream::decode_all(bytes).context("decompress scrollback")?;
        let mut reader = raw.as_slice();
        let mut decoder = varbincode::Deserializer::new(&mut reader);
        let mut saved = Self::deserialize(&mut decoder).context("deserialize scrollback")?;
        anyhow::ensure!(
            saved.version == VERSION,
            "scrollback file is version {}, this build reads {VERSION}",
            saved.version
        );
        saved.relink_hyperlinks();
        Ok(saved)
    }

    /// serde gives every cell its own `Arc<Hyperlink>`; hover highlighting
    /// and the mux protocol expect a run to share one, so re-share by value.
    fn relink_hyperlinks(&mut self) {
        let mut prev: Option<Arc<Hyperlink>> = None;
        for line in &mut self.lines {
            for cell in line.cells_mut_for_attr_changes_only() {
                let Some(link) = cell.attrs().hyperlink().map(Arc::clone) else {
                    prev = None;
                    continue;
                };
                match &prev {
                    Some(p) if **p == *link => {
                        cell.attrs_mut().set_hyperlink(Some(Arc::clone(p)));
                    }
                    _ => prev = Some(link),
                }
            }
        }
    }

    /// Write atomically (temp file + rename), owner-readable only.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let bytes = self.encode()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        file.write_all(&bytes)?;
        file.flush()?;
        drop(file);
        std::fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::decode(&bytes)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use termwiz::surface::SEQ_ZERO;
    use wezterm_term::CellAttributes;

    /// Text, attributes, the soft-wrap bit and hyperlinks survive a round
    /// trip, and a hyperlink run shares one Arc again afterwards.
    #[test]
    fn round_trip_relinks_hyperlinks() {
        let link = Arc::new(Hyperlink::new("https://example.com"));
        let linked = CellAttributes::default()
            .set_hyperlink(Some(Arc::clone(&link)))
            .clone();
        let mut wrapped = Line::from_text("abc", &CellAttributes::default(), SEQ_ZERO, None);
        wrapped.set_last_cell_was_wrapped(true, SEQ_ZERO);
        let saved = SavedScrollback::new(
            80,
            vec![
                Line::from_text("see this", &linked, SEQ_ZERO, None),
                wrapped,
            ],
        );

        let loaded = SavedScrollback::decode(&saved.encode().unwrap()).unwrap();
        assert_eq!(loaded.cols, 80);
        assert_eq!(loaded.lines.len(), 2);
        assert_eq!(loaded.lines[0].as_str(), "see this");
        assert!(loaded.lines[1].last_cell_was_wrapped());
        let links: Vec<Arc<Hyperlink>> = loaded.lines[0]
            .visible_cells()
            .filter_map(|c| c.attrs().hyperlink().map(Arc::clone))
            .collect();
        assert_eq!(links.len(), 8);
        assert_eq!(links[0].uri(), "https://example.com");
        assert!(links.iter().all(|l| Arc::ptr_eq(l, &links[0])));
    }

    #[test]
    fn rejects_other_versions() {
        let mut saved = SavedScrollback::new(1, vec![]);
        saved.version = VERSION + 1;
        assert!(SavedScrollback::decode(&saved.encode().unwrap()).is_err());
    }
}
