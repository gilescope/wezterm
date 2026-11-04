use crate::pane::CloseReason;
use crate::{Mux, MuxNotification, Tab, TabId};
use config::GuiPosition;
use std::ops::Range;
use std::sync::Arc;

static WIN_ID: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::AtomicUsize::new(0);
pub type WindowId = usize;

pub struct Window {
    id: WindowId,
    tabs: Vec<Arc<Tab>>,
    active_tab_idx: usize,
    last_active_tab_id: Option<TabId>,
    /// Tabs the user switched to, oldest first, by id so a reorder or a
    /// close cannot point it at the wrong tab. `history_pos` is the entry
    /// we are at; later entries are what "forward" returns to.
    tab_history: Vec<TabId>,
    history_pos: usize,
    workspace: String,
    title: String,
    initial_position: Option<GuiPosition>,
}

impl Window {
    /// Create a new Window.
    pub fn new(workspace: Option<String>, initial_position: Option<GuiPosition>) -> Self {
        Self {
            id: WIN_ID.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed),
            tabs: vec![],
            active_tab_idx: 0,
            last_active_tab_id: None,
            tab_history: vec![],
            history_pos: 0,
            title: String::new(),
            workspace: workspace.unwrap_or_else(|| Mux::get().active_workspace()),
            initial_position,
        }
    }

    /// Return initial position of the window.
    pub fn get_initial_position(&self) -> &Option<GuiPosition> {
        &self.initial_position
    }

    /// Return current workspace.
    pub fn get_workspace(&self) -> &str {
        &self.workspace
    }

    /// Set window title, notifying listeners if it changed.
    pub fn set_title(&mut self, title: &str) {
        if self.title != title {
            self.title = title.to_string();
            Mux::try_get().map(|mux| {
                mux.notify(MuxNotification::WindowTitleChanged {
                    window_id: self.id,
                    title: title.to_string(),
                })
            });
        }
    }

    /// Return current title.
    pub fn get_title(&self) -> &str {
        &self.title
    }

    /// Set window workspace, notifying listeners if it changed.
    pub fn set_workspace(&mut self, workspace: &str) {
        if workspace == self.workspace {
            return;
        }
        self.workspace = workspace.to_string();
        Mux::get().notify(MuxNotification::WindowWorkspaceChanged(self.id));
    }

    pub fn window_id(&self) -> WindowId {
        self.id
    }

    /// Panic if given tab is already present in this window.
    fn assert_tab_isnt_already_in_window(&self, tab: &Arc<Tab>) {
        for t in &self.tabs {
            assert_ne!(t.tab_id(), tab.tab_id(), "tab already added to this window");
        }
    }

    /// Notify that this window's tab layout has changed (add/remove/reorder),
    /// so listeners can refresh any cached view of it.
    fn invalidate(&self) {
        if let Some(mux) = Mux::try_get() {
            mux.notify(MuxNotification::WindowInvalidated(self.id));
        }
    }

    /// Index of the root tab that `idx` belongs to: itself, or its parent.
    /// A child whose parent is not in this window counts as a root.
    pub fn root_idx_of(&self, idx: usize) -> usize {
        self.tabs
            .get(idx)
            .and_then(|t| t.get_parent())
            .and_then(|parent| self.get_tab_idx_for_id(parent))
            .unwrap_or(idx)
    }

    /// Indices of the children of the root at `root_idx`. Children are kept
    /// contiguous right after their root, so this is a plain range.
    pub fn children_of(&self, root_idx: usize) -> Range<usize> {
        let Some(root) = self.tabs.get(root_idx) else {
            return root_idx..root_idx;
        };
        let root_id = root.tab_id();
        let end = self.tabs[root_idx + 1..]
            .iter()
            .position(|t| t.get_parent() != Some(root_id))
            .map(|n| root_idx + 1 + n)
            .unwrap_or(self.tabs.len());
        root_idx + 1..end
    }

    /// Indices of the root tabs, in order.
    pub fn root_indices(&self) -> Vec<usize> {
        (0..self.tabs.len())
            .filter(|&i| self.root_idx_of(i) == i)
            .collect()
    }

    /// What a vertical bar shows: the root indices for the side bar and the
    /// active root's block (root plus its children) for the row across the
    /// top. Indices are into this window's tab list.
    pub fn bar_layout(&self) -> (Vec<usize>, Range<usize>) {
        let root = self.root_idx_of(self.active_tab_idx);
        (self.root_indices(), root..self.children_of(root).end)
    }

    /// Make `tab_id` a child of `parent_id` and move it to the end of the
    /// parent's block. Returns its new index. A tab cannot nest under a
    /// child (one level only) or under itself.
    pub fn attach_tab_to_parent(&mut self, tab_id: TabId, parent_id: TabId) -> Option<usize> {
        let idx = self.get_tab_idx_for_id(tab_id)?;
        let parent_idx = self.get_tab_idx_for_id(parent_id)?;
        if tab_id == parent_id || self.root_idx_of(parent_idx) != parent_idx {
            return None;
        }
        let active = self.get_active_tab().map(Arc::clone);
        let tab = self.tabs.remove(idx);
        tab.set_parent(Some(parent_id));
        let parent_idx = self.get_tab_idx_for_id(parent_id)?;
        let dest = self.children_of(parent_idx).end;
        self.tabs.insert(dest, tab);
        self.fixup_active_tab_after_removal(active);
        Some(dest)
    }

    /// Move the tab at `idx` by `delta` steps: a root steps over whole
    /// root blocks and carries its children; a child moves within its
    /// block. Returns the tab's new index, or `None` when nothing moved.
    pub fn move_tab_relative(&mut self, idx: usize, delta: isize) -> Option<usize> {
        if delta == 0 || idx >= self.tabs.len() {
            return None;
        }
        let active = self.get_active_tab().map(Arc::clone);
        let root_idx = self.root_idx_of(idx);
        let new_idx = if root_idx == idx {
            let roots = self.root_indices();
            let r = roots.iter().position(|&i| i == idx)?;
            let target = r.saturating_add_signed(delta).min(roots.len() - 1);
            if target == r {
                return None;
            }
            let block: Vec<Arc<Tab>> = self.tabs.drain(idx..self.children_of(idx).end).collect();
            let dest = if target > r {
                // The target root shifted down by one block after the drain.
                let t = self.root_indices()[target - 1];
                self.children_of(t).end
            } else {
                self.root_indices()[target]
            };
            for (n, t) in block.into_iter().enumerate() {
                self.tabs.insert(dest + n, t);
            }
            dest
        } else {
            let range = self.children_of(root_idx);
            let dest = idx
                .saturating_add_signed(delta)
                .clamp(range.start, range.end - 1);
            if dest == idx {
                return None;
            }
            let tab = self.tabs.remove(idx);
            self.tabs.insert(dest, tab);
            dest
        };
        self.fixup_active_tab_after_removal(active);
        Some(new_idx)
    }

    /// Insert `tab` in window, at given index.
    pub fn insert_tab_at_idx(&mut self, index: usize, tab: &Arc<Tab>) {
        self.assert_tab_isnt_already_in_window(tab);
        self.tabs.insert(index, Arc::clone(tab));
        self.invalidate();
    }

    /// Insert `tab` in window, after other tabs (or, for a tab that already
    /// has a parent here, at the end of that parent's block).
    pub fn push_tab(&mut self, tab: &Arc<Tab>) {
        self.assert_tab_isnt_already_in_window(tab);
        let dest = tab
            .get_parent()
            .and_then(|p| self.get_tab_idx_for_id(p))
            .map(|p| self.children_of(p).end)
            .unwrap_or(self.tabs.len());
        self.tabs.insert(dest, Arc::clone(tab));
        self.invalidate();
    }

    /// Return true if this window contains no tabs.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Return number of tabs in this window.
    pub fn count_tabs(&self) -> usize {
        self.tabs.len()
    }

    /// Return tab at given index, if any.
    pub fn get_tab_at_idx(&self, idx: usize) -> Option<&Arc<Tab>> {
        self.tabs.get(idx)
    }

    /// Return true if every tab can be closed without prompting the user.
    pub fn can_close_without_prompting(&self) -> bool {
        for tab in &self.tabs {
            if !tab.can_close_without_prompting(CloseReason::Window) {
                return false;
            }
        }
        true
    }

    /// Return index of tab with given id, if it is found in this window.
    pub fn get_tab_idx_for_id(&self, id: TabId) -> Option<usize> {
        for (idx, t) in self.tabs.iter().enumerate() {
            if t.tab_id() == id {
                return Some(idx);
            }
        }
        None
    }

    /// Re-establish a valid active tab after a tab removal.
    fn fixup_active_tab_after_removal(&mut self, active: Option<Arc<Tab>>) {
        let len = self.tabs.len();
        if let Some(active) = active {
            for (idx, tab) in self.tabs.iter().enumerate() {
                if tab.tab_id() == active.tab_id() {
                    self.set_active_tab_idx_without_saving(idx);
                    return;
                }
            }
        }

        if len > 0 && self.active_tab_idx >= len {
            self.set_active_tab_idx_without_saving(len - 1);
        } else {
            self.invalidate();
        }
    }

    /// Remove tab at given index and return it.
    pub fn remove_tab_idx(&mut self, idx: usize) -> Arc<Tab> {
        self.invalidate();
        let active = self.get_active_tab().map(Arc::clone);
        self.do_remove_tab_idx(idx, active)
    }

    /// Remove tab with given id, if present.
    pub fn remove_tab_id(&mut self, id: TabId) {
        let active = self.get_active_tab().map(Arc::clone);
        if let Some(idx) = self.get_tab_idx_for_id(id) {
            self.do_remove_tab_idx(idx, active);
        }
    }

    /// Remove tab at given index, switching to previous active tab if needed.
    fn do_remove_tab_idx(&mut self, idx: usize, active: Option<Arc<Tab>>) -> Arc<Tab> {
        if let (Some(active), Some(removing)) = (&active, self.tabs.get(idx)) {
            if active.tab_id() == removing.tab_id()
                && config::configuration().switch_to_last_active_tab_when_closing_tab
            {
                // If we are removing active tab, switch back to previously active tab
                if let Some(last_active) = self.get_last_active_tab_idx() {
                    self.set_active_tab_idx_without_saving(last_active);
                }
            }
        }
        let tab = self.tabs.remove(idx);
        // A root's children outlive it as roots, in place.
        for child in self
            .tabs
            .iter()
            .filter(|t| t.get_parent() == Some(tab.tab_id()))
        {
            child.set_parent(None);
        }
        self.fixup_active_tab_after_removal(active);
        tab
    }

    /// Return currently active tab.
    pub fn get_active_tab(&self) -> Option<&Arc<Tab>> {
        // FIXME: `self.active_tab_idx` is supposed to be always valid, so this should really always
        // return the Tab.. and warn/panic(?) if active tab index isn't valid somehow (logic error!)
        self.get_tab_at_idx(self.active_tab_idx)
    }

    /// Return index of currently active tab.
    #[inline]
    pub fn get_active_tab_idx(&self) -> usize {
        self.active_tab_idx
    }

    /// Remember current tab as the "last active" tab.
    pub fn remember_current_as_last_active_tab(&mut self) {
        self.last_active_tab_id = self
            .get_tab_at_idx(self.active_tab_idx)
            .map(|tab| tab.tab_id());
    }

    /// Return index of previously active tab, if any.
    #[inline]
    pub fn get_last_active_tab_idx(&self) -> Option<usize> {
        if let Some(tab_id) = self.last_active_tab_id {
            self.get_tab_idx_for_id(tab_id)
        } else {
            None
        }
    }

    /// Remember current tab as the "last active" tab, then make tab at given index active.
    /// No-op when given index is already the active tab.
    pub fn remember_and_set_active_tab_idx(&mut self, idx: usize) {
        if idx == self.get_active_tab_idx() {
            return;
        }
        self.remember_current_as_last_active_tab();
        self.record_history(idx);
        self.set_active_tab_idx_without_saving(idx);
    }

    /// Entries a walk keeps, oldest dropped first.
    const HISTORY_CAP: usize = 100;

    /// Note a user switch to `idx` in the browser-style history: the forward
    /// entries are dropped, as a browser does when you navigate afresh.
    fn record_history(&mut self, idx: usize) {
        self.sync_history_to_active();
        if let Some(tab) = self.tabs.get(idx) {
            self.push_history(tab.tab_id());
        }
    }

    /// The window can move on its own (the active tab closed), so the entry
    /// at `history_pos` is not always where we are. Splice the active tab in
    /// after it, keeping the forward entries, so a walk from here includes
    /// the tab being left.
    fn sync_history_to_active(&mut self) {
        let Some(cur) = self.tabs.get(self.active_tab_idx).map(|t| t.tab_id()) else {
            return;
        };
        if self.tab_history.get(self.history_pos) == Some(&cur) {
            return;
        }
        if !self.tab_history.is_empty() {
            self.history_pos += 1;
        }
        self.tab_history.insert(self.history_pos, cur);
        if self.tab_history.len() > Self::HISTORY_CAP {
            self.tab_history.remove(0);
            self.history_pos -= 1;
        }
    }

    fn push_history(&mut self, id: TabId) {
        if !self.tab_history.is_empty() {
            self.tab_history.truncate(self.history_pos + 1);
        }
        if self.tab_history.last() != Some(&id) {
            self.tab_history.push(id);
        }
        if self.tab_history.len() > Self::HISTORY_CAP {
            self.tab_history.remove(0);
        }
        self.history_pos = self.tab_history.len() - 1;
    }

    /// The history position `delta` steps away (negative = back) that names
    /// a live tab other than the active one. Closed tabs stay in the list
    /// and are stepped over, so no pruning is needed.
    fn history_target(&self, delta: isize) -> Option<(usize, usize)> {
        let step = delta.signum();
        let mut pos = self.history_pos;
        for _ in 0..delta.unsigned_abs() {
            loop {
                pos = pos.checked_add_signed(step)?;
                let idx = self.get_tab_idx_for_id(*self.tab_history.get(pos)?);
                match idx {
                    Some(idx) if idx != self.active_tab_idx => break,
                    _ => {}
                }
            }
        }
        let idx = self.get_tab_idx_for_id(self.tab_history[pos])?;
        Some((pos, idx))
    }

    /// Whether a back (negative) or forward step has somewhere to go.
    pub fn can_step_history(&self, delta: isize) -> bool {
        self.history_target(delta).is_some()
    }

    /// Browser-style back/forward: activate the tab `delta` steps away in
    /// the history without recording a new entry. Returns its index.
    pub fn history_step(&mut self, delta: isize) -> Option<usize> {
        self.sync_history_to_active();
        let (pos, idx) = self.history_target(delta)?;
        self.history_pos = pos;
        self.remember_current_as_last_active_tab();
        self.set_active_tab_idx_without_saving(idx);
        Some(idx)
    }

    /// Make given index the active tab without remembering current tab as the "last active" tab.
    pub fn set_active_tab_idx_without_saving(&mut self, idx: usize) {
        assert!(idx < self.tabs.len());
        if self.active_tab_idx != idx {
            if let Some(tab) = self.tabs.get(self.active_tab_idx) {
                if let Some(pane) = tab.get_active_pane() {
                    pane.focus_changed(false);
                }
            }
        }
        self.active_tab_idx = idx;
        self.invalidate();
    }

    /// Iterate over tabs in this window.
    pub fn iter_tabs(&self) -> impl Iterator<Item = &Arc<Tab>> {
        self.tabs.iter()
    }

    /// Remove dead tabs, and any tabs not present in given `live_tab_ids`.
    pub fn prune_dead_tabs(&mut self, live_tab_ids: &[TabId]) {
        let mut invalidated = false;
        let dead: Vec<TabId> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                if tab.prune_dead_panes() {
                    invalidated = true;
                }
                if tab.is_dead() {
                    Some(tab.tab_id())
                } else {
                    None
                }
            })
            .collect();

        for tab_id in dead {
            log::trace!("Window::prune_dead_tabs: tab_id {} is dead", tab_id);
            self.remove_tab_id(tab_id);
            invalidated = true;
        }

        let dead: Vec<TabId> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                if live_tab_ids
                    .iter()
                    .find(|&&id| id == tab.tab_id())
                    .is_none()
                {
                    Some(tab.tab_id())
                } else {
                    None
                }
            })
            .collect();
        for tab_id in dead {
            log::trace!("Window::prune_dead_tabs: (live) tab_id {} is dead", tab_id);
            self.remove_tab_id(tab_id);
        }

        if invalidated {
            self.invalidate();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use wezterm_term::TerminalSize;

    fn tab() -> Arc<Tab> {
        Arc::new(Tab::new(&TerminalSize::default()))
    }

    fn window_with(n: usize) -> (Window, Vec<Arc<Tab>>) {
        let mut w = Window::new(Some("test".into()), None);
        let tabs: Vec<_> = (0..n).map(|_| tab()).collect();
        for t in &tabs {
            w.push_tab(t);
        }
        (w, tabs)
    }

    fn ids(w: &Window) -> Vec<TabId> {
        w.iter_tabs().map(|t| t.tab_id()).collect()
    }

    /// Children sit directly after their root, in attach order; a later
    /// root goes after the whole block.
    #[test]
    fn children_are_contiguous_after_their_root() {
        let (mut w, t) = window_with(2);
        let c1 = tab();
        let c2 = tab();
        w.push_tab(&c1);
        w.push_tab(&c2);
        w.attach_tab_to_parent(c1.tab_id(), t[0].tab_id());
        w.attach_tab_to_parent(c2.tab_id(), t[0].tab_id());
        assert_eq!(
            ids(&w),
            vec![t[0].tab_id(), c1.tab_id(), c2.tab_id(), t[1].tab_id()]
        );
        assert_eq!(c1.get_parent(), Some(t[0].tab_id()));
        assert_eq!(w.root_idx_of(2), 0);
        assert_eq!(w.root_idx_of(3), 3);
        assert_eq!(w.children_of(0), 1..3);
        assert_eq!(w.children_of(3), 4..4);
        assert_eq!(w.root_indices(), vec![0, 3]);

        // A root pushed later lands after the block, not inside it.
        let t3 = tab();
        w.push_tab(&t3);
        assert_eq!(ids(&w)[4], t3.tab_id());
    }

    /// Removing a root promotes its children to roots, in place.
    #[test]
    fn removing_a_root_promotes_its_children() {
        let (mut w, t) = window_with(2);
        let c = tab();
        w.push_tab(&c);
        w.attach_tab_to_parent(c.tab_id(), t[0].tab_id());
        w.remove_tab_idx(0);
        assert_eq!(ids(&w), vec![c.tab_id(), t[1].tab_id()]);
        assert_eq!(c.get_parent(), None);
        assert_eq!(w.root_indices(), vec![0, 1]);
    }

    /// A root moves with its children; a child moves only within its block.
    #[test]
    fn move_tab_relative_respects_blocks() {
        let (mut w, t) = window_with(3);
        let c1 = tab();
        let c2 = tab();
        w.push_tab(&c1);
        w.push_tab(&c2);
        w.attach_tab_to_parent(c1.tab_id(), t[0].tab_id());
        w.attach_tab_to_parent(c2.tab_id(), t[0].tab_id());
        // [t0 c1 c2 t1 t2]
        assert_eq!(w.move_tab_relative(0, 1), Some(1));
        assert_eq!(
            ids(&w),
            vec![
                t[1].tab_id(),
                t[0].tab_id(),
                c1.tab_id(),
                c2.tab_id(),
                t[2].tab_id()
            ]
        );
        assert_eq!(w.move_tab_relative(1, 1), Some(2));
        assert_eq!(
            ids(&w),
            vec![
                t[1].tab_id(),
                t[2].tab_id(),
                t[0].tab_id(),
                c1.tab_id(),
                c2.tab_id()
            ]
        );
        // Already last root: no move.
        assert_eq!(w.move_tab_relative(2, 1), None);
        // Child moves within the block and no further.
        assert_eq!(w.move_tab_relative(3, 1), Some(4));
        assert_eq!(ids(&w)[3..], [c2.tab_id(), c1.tab_id()]);
        assert_eq!(w.move_tab_relative(4, 1), None);
        assert_eq!(w.move_tab_relative(3, -1), None);
        // Back to the front as a block.
        assert_eq!(w.move_tab_relative(2, -2), Some(0));
        assert_eq!(
            ids(&w),
            vec![
                t[0].tab_id(),
                c2.tab_id(),
                c1.tab_id(),
                t[1].tab_id(),
                t[2].tab_id()
            ]
        );
    }

    /// A(a1 a2) B(b1) C: whichever tab is active, the row is its root's
    /// block and the side bar's active root is that root.
    #[test]
    fn bar_layout_follows_the_active_tab() {
        let (mut w, t) = window_with(3);
        let (a1, a2, b1) = (tab(), tab(), tab());
        for (child, root) in [(&a1, &t[0]), (&a2, &t[0]), (&b1, &t[1])] {
            w.push_tab(child);
            w.attach_tab_to_parent(child.tab_id(), root.tab_id());
        }
        // Order: A a1 a2 B b1 C
        let expect = [
            (0, 0..3),
            (0, 0..3),
            (0, 0..3),
            (3, 3..5),
            (3, 3..5),
            (5, 5..6),
        ];
        for (idx, (root, block)) in expect.iter().enumerate() {
            w.remember_and_set_active_tab_idx(idx);
            let (roots, row) = w.bar_layout();
            assert_eq!(roots, vec![0, 3, 5], "roots with tab {idx} active");
            assert_eq!(w.root_idx_of(idx), *root, "root of tab {idx}");
            assert_eq!(row, *block, "row block with tab {idx} active");
            assert!(row.contains(&idx), "row holds the active tab {idx}");
        }
    }
}

#[cfg(test)]
mod history_test {
    use super::*;
    use wezterm_term::TerminalSize;

    fn window_with(n: usize) -> Window {
        let mut w = Window::new(Some("test".into()), None);
        for _ in 0..n {
            w.push_tab(&Arc::new(Tab::new(&TerminalSize::default())));
        }
        w
    }

    /// Back walks to earlier tabs, forward returns, and a fresh activation
    /// drops the forward entries, as a browser's history does.
    #[test]
    fn history_steps_back_and_forward_like_a_browser() {
        let mut w = window_with(4);
        assert!(!w.can_step_history(-1));
        w.remember_and_set_active_tab_idx(1);
        w.remember_and_set_active_tab_idx(2);
        assert!(w.can_step_history(-1));
        assert!(!w.can_step_history(1));
        assert_eq!(w.history_step(-1), Some(1));
        assert_eq!(w.get_active_tab_idx(), 1);
        assert_eq!(w.history_step(-1), Some(0));
        assert_eq!(w.history_step(-1), None);
        assert!(w.can_step_history(1));
        assert_eq!(w.history_step(1), Some(1));
        w.remember_and_set_active_tab_idx(3);
        assert!(!w.can_step_history(1));
        assert_eq!(w.history_step(-1), Some(1));
        assert_eq!(w.history_step(-1), Some(0));
        // ActivateLastTab still toggles with the tab we just left.
        assert_eq!(w.get_last_active_tab_idx(), Some(1));
    }

    /// A closed tab drops out of the walk; its neighbours still connect.
    #[test]
    fn history_skips_closed_tabs() {
        let mut w = window_with(3);
        w.remember_and_set_active_tab_idx(1);
        w.remember_and_set_active_tab_idx(2);
        w.remove_tab_idx(1);
        assert_eq!(w.get_active_tab_idx(), 1);
        assert_eq!(w.history_step(-1), Some(0));
        assert!(w.can_step_history(1));
        assert_eq!(w.history_step(1), Some(1));
        assert!(!w.can_step_history(1));
    }

    /// Re-activating the current tab, or a tab the window switched to on
    /// its own (closing the active one), does not leave the walk stranded.
    #[test]
    fn history_tracks_where_the_window_actually_is() {
        let mut w = window_with(3);
        w.remember_and_set_active_tab_idx(1);
        w.remember_and_set_active_tab_idx(1);
        w.remove_tab_idx(1);
        // The window moved to the next tab by itself (not a user step), so
        // the history still ends at the closed tab: back goes to tab 0, and
        // a fresh activation first records the tab we are really leaving.
        assert_eq!(w.get_active_tab_idx(), 1);
        assert_eq!(w.history_step(-1), Some(0));
        assert_eq!(w.history_step(1), Some(1));
        w.remember_and_set_active_tab_idx(0);
        assert_eq!(w.history_step(-1), Some(1));
        assert_eq!(w.history_step(-1), Some(0));
        assert_eq!(w.history_step(-1), None);
    }
}
