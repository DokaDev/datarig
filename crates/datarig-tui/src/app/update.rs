//! Input handling: terminal events (keys, paste, mouse) routed to the focused pane, dialog or
//! screen, plus background [`AppEvent`]s.

use super::explorer::RowKind;
use super::*;
use crate::widgets::editor::RegProblem;
use crate::widgets::grid::Shape;
use crate::widgets::tree::Reveal;

impl App {
    pub fn handle_event(&mut self, ev: Event) {
        // A mouse move that is not a drag (the terminal reports every motion while the mouse is
        // captured) only selects the item under the pointer in an open menu or the help; any
        // other move, and one that stays on the selected item, changes nothing on screen: the
        // event loop draws no frame for it.
        if let Event::Mouse(m) = ev
            && m.kind == MouseEventKind::Moved
        {
            let hovered = match self.overlays.top().map(|o| o.kind()) {
                Some(OverlayKind::ContextMenu) => self.menu_hover(m),
                Some(OverlayKind::Help) => self.help_hover(m),
                _ => false,
            };
            self.idle_event = !hovered;
            return;
        }
        match ev {
            Event::Key(k) => {
                let k = keyboard::normalize(k);
                match keyboard::key_use(&k) {
                    KeyUse::Ignore => {}
                    u => self.handle_key(k, u == KeyUse::Repeat),
                }
            }
            Event::Mouse(m) if self.overlays.top().is_some_and(|o| o.kind() == OverlayKind::Help) => self.help_mouse(m),
            Event::Mouse(m) if self.overlays.top().is_some_and(|o| o.kind() == OverlayKind::ScriptTree) => {
                self.script_tree_mouse(m)
            }
            Event::Mouse(m) if self.overlays.top().is_some_and(|o| o.kind() == OverlayKind::ProfileForm) => {
                self.form_mouse(m)
            }
            Event::Mouse(m) if self.overlays.top().is_some_and(|o| o.kind() == OverlayKind::ContextMenu) => {
                self.menu_mouse(m)
            }
            Event::Mouse(m) if !self.modal_open() => self.handle_mouse(m),
            Event::Paste(text) => self.paste(&text),
            _ => {}
        }
        self.after_input();
    }

    /// The last input event changed nothing (a mouse move): no frame is needed for it.
    pub fn take_idle_event(&mut self) -> bool {
        std::mem::take(&mut self.idle_event)
    }

    pub fn on_app_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Db { target, generation, ev } if self.is_current(target, generation) => {
                self.on_target_event(target, ev)
            }
            AppEvent::Db { .. } => {}
            AppEvent::Ping { seq, result } => self.on_ping(seq, result),
            AppEvent::Migrated(report) => self.on_migrated(report),
            AppEvent::Resolved { profile, generation, result } if self.conns.is_current(profile, generation) => {
                self.on_resolved(profile, result)
            }
            AppEvent::Resolved { .. } => {}
            AppEvent::TestSource { seq, error } => self.on_test_source(seq, error),
            AppEvent::KeychainProbed(r) => self.on_keychain_probed(r),
            AppEvent::Keychain(done) => self.on_keychain_done(done),
            AppEvent::Tunnel { profile, generation, ev } => self.on_tunnel_event(profile, generation, ev),
            AppEvent::TestTunnel { seq, ev } => self.on_test_tunnel_event(seq, ev),
        }
        // What an open quick connect list waits for may have arrived.
        if self.overlays.quick().is_some() {
            self.refresh_quick();
        }
    }

    pub(super) fn paste(&mut self, text: &str) {
        if self.overlays.top().is_some_and(|o| {
            matches!(
                o.kind(),
                OverlayKind::Busy | OverlayKind::Confirm | OverlayKind::RunConfirm | OverlayKind::IconsAsk
            )
        }) {
            return;
        }
        if let Some(c) = self.overlays.command_line_mut() {
            c.input.insert_str(text);
            self.refresh_commands();
        } else if let Some(h) = self.overlays.help_mut().filter(|h| h.filtering) {
            h.filter.insert_str(text);
            self.help_select_first_entry();
        } else if let Some(p) = self.overlays.prompt_mut() {
            if !p.save_focus {
                p.input.insert_str(text);
            }
        } else if let Some(q) = self.overlays.quick_mut() {
            q.input.insert_str(text);
            q.selected = 0;
            q.items.clear();
            self.refresh_quick();
        } else if let Some(t) = self.overlays.script_tree_mut().filter(|t| t.focus == script_tree::TreeFocus::Name) {
            t.input.insert_str(text);
            t.error = None;
        } else if let Some(n) = self.overlays.name_input_mut() {
            n.input.insert_str(text);
            n.error = None;
        } else if let Some(c) = self.overlays.chooser_mut().filter(|c| c.filtering) {
            c.filter.insert_str(text);
            c.selected = 0;
        } else if self.overlays.is_open(OverlayKind::Chooser) {
            // A list, not text: nothing to paste into.
        } else if let Some(f) = self.overlays.form_mut() {
            f.paste(text);
        } else if self.focus == Focus::Tree && self.explorer.filtering && self.overlays.is_empty() {
            self.explorer.filter.insert_str(text);
        } else if self.overlays.is_empty() && (self.focus == Focus::Tree || self.profiles.is_empty()) {
            // The explorer or the welcome panel: a connection URL makes a new profile.
            self.paste_dsn(text);
        } else if self.overlays.is_empty() && self.focus == Focus::Editor && !self.profiles.is_empty() {
            let t = self.tab_mut();
            t.editor.paste(text);
            t.popup = None;
            self.edited();
        }
    }

    /// Resolve a key in the current context. `repeat`: terminal auto-repeat, which
    /// runs only repeatable actions (movement) but still types and edits in widgets.
    pub(super) fn handle_key(&mut self, key: KeyEvent, repeat: bool) {
        let chord = KeyChord::from_event(&key);
        let ctx = self.key_context();
        // The completion popup takes its keys before the editor.
        if self.tab().popup.is_some()
            && self.overlays.is_empty()
            && ctx == Ctx::VimInsert
            && self.keymap.binds(Ctx::Completion, &chord)
        {
            self.key_state.clear();
            self.popup_key(key);
            return;
        }
        // Outside text input, Hangul typed with a Korean input source means the QWERTY keys at
        // the same places; not the character a vim command waits for (`f`, `t`, `r`), which is
        // taken as it is typed. The register after `Ctrl+R` in Insert mode is a key too.
        let literal = matches!(ctx, Ctx::VimNormal | Ctx::VimVisual) && self.tab().editor.awaiting_char();
        let register = ctx == Ctx::VimInsert && self.tab().editor.awaiting_key();
        let mapped = match chord.code {
            KeyCode::Char(c) if chord.is_plain_char() && (!ctx.is_text_input() || register) && !literal => {
                hangul::keys(c)
            }
            _ => None,
        };
        match mapped {
            Some(keys) => {
                self.hangul_hint = Some(Instant::now());
                // A syllable is several keys; each is resolved where the previous one left off.
                for k in keys {
                    let ctx = self.key_context();
                    self.feed_key(ctx, KeyChord::char(k), repeat);
                }
            }
            None => self.feed_key(ctx, chord, repeat),
        }
    }

    fn feed_key(&mut self, ctx: Ctx, k: KeyChord, repeat: bool) {
        // A vim command waiting for its argument (`d…`, `f…`) gets the key, unless it is an app key
        // of the workspace (run, commands, quit, …).
        let rctx = if matches!(ctx, Ctx::VimNormal | Ctx::VimVisual) && self.tab().editor.awaiting_key() {
            Ctx::Workspace
        } else {
            ctx
        };
        let r = self.keymap.feed(&mut self.key_state, rctx, k);
        self.track_leader(r == Resolved::Pending && !repeat);
        match r {
            Resolved::Action(a) => {
                if !repeat || action::spec(a).repeatable {
                    self.dispatch(a);
                }
            }
            // A held key does not start a sequence.
            Resolved::Pending if repeat => self.key_state.clear(),
            Resolved::Pending => {}
            Resolved::Unbound(seq) => self.leader_unbound(&seq),
            Resolved::Forward(keys) => self.forward(ctx, &keys, repeat),
        }
    }

    /// Keys no action claims go to the widget of context `ctx`.
    fn forward(&mut self, ctx: Ctx, keys: &[KeyChord], repeat: bool) {
        match ctx {
            Ctx::VimNormal | Ctx::VimVisual | Ctx::VimInsert => {
                for k in keys {
                    self.editor_key(k.to_event());
                }
            }
            Ctx::Commands => keys.iter().for_each(|k| self.command_key(k.to_event(), repeat)),
            Ctx::Password => keys.iter().for_each(|k| self.prompt_key(k.to_event(), repeat)),
            Ctx::ExplorerFilter => keys.iter().for_each(|k| self.explorer_filter_key(k.to_event())),
            Ctx::QuickConnect => keys.iter().for_each(|k| self.quick_key(k.to_event(), repeat)),
            Ctx::Chooser | Ctx::ChooserFilter => keys.iter().for_each(|k| self.chooser_key(k.to_event(), repeat)),
            Ctx::NameInput => keys.iter().for_each(|k| self.name_key(k.to_event(), repeat)),
            Ctx::ScriptTree | Ctx::ScriptTreeName => {
                keys.iter().for_each(|k| self.script_tree_key(k.to_event(), repeat))
            }
            Ctx::ContextMenu => keys.iter().for_each(|k| self.menu_key(*k, repeat)),
            Ctx::Confirm => keys.iter().for_each(|k| self.confirm_key(k.to_event(), repeat)),
            Ctx::RunConfirm => keys.iter().for_each(|k| self.run_confirm_key(k.to_event(), repeat)),
            Ctx::IconsAsk => keys.iter().for_each(|k| self.icons_ask_key(k.to_event(), repeat)),
            Ctx::ProfileForm => keys.iter().for_each(|k| self.form_key(k.to_event(), repeat)),
            Ctx::Settings => keys.iter().for_each(|k| self.settings_key(k.to_event(), repeat)),
            Ctx::CellViewer => self.viewer_keys(keys),
            Ctx::WhichKey => keys.iter().for_each(|k| self.which_key_key(*k, repeat)),
            Ctx::Help => self.help_keys(keys, repeat),
            Ctx::HelpFilter => keys.iter().for_each(|k| self.help_filter_key(k.to_event())),
            _ => {}
        }
    }

    pub(super) fn prompt_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(p) = self.overlays.prompt_mut() else { return };
        let savable = p.save_to.is_some();
        match key.code {
            KeyCode::Esc => self.prompt_closed(),
            KeyCode::Enter if !repeat => self.prompt_entered(),
            KeyCode::Tab | KeyCode::BackTab if !repeat && savable => p.save_focus = !p.save_focus,
            KeyCode::Char(' ') if p.save_focus => {
                if !repeat {
                    p.save = !p.save;
                }
            }
            _ if p.save_focus => {}
            _ => {
                p.input.handle_key(&key);
            }
        }
    }

    /// The mouse on the profile form: a click on the key file field's `[…]` button opens the
    /// key file picker.
    pub(super) fn form_mouse(&mut self, m: MouseEvent) {
        let Some(f) = self.overlays.form() else { return };
        let b = f.key_button;
        let on = m.column >= b.x && m.column < b.x + b.width && m.row >= b.y && m.row < b.y + b.height;
        if m.kind == MouseEventKind::Down(MouseButton::Left) && on && !f.saving {
            self.open_key_picker();
        }
    }

    /// Keys of the profile form.
    pub(super) fn form_key(&mut self, key: KeyEvent, repeat: bool) {
        let test_running = self.conn_test.as_ref().is_some_and(|t| t.state == TestState::Running);
        // A save waits for the keychain (at most its time limit): keys wait too.
        let Some(f) = self.overlays.form_mut().filter(|f| !f.saving) else { return };
        if key.code == KeyCode::Esc && test_running {
            self.cancel_test();
            return;
        }
        let out = f.key(&key);
        if repeat && out != FormOutcome::None {
            return;
        }
        match out {
            FormOutcome::None => {}
            FormOutcome::Save => self.save_form(),
            FormOutcome::Test => self.test_in_context(None),
            FormOutcome::Choose(f) => self.open_form_chooser(f),
            FormOutcome::Cancel => {
                self.overlays.close(OverlayKind::ProfileForm);
                self.clear_test();
            }
        }
    }

    /// Move the focus to the next (`d = 1`) or previous pane. Without profiles there are two
    /// panes: the explorer and the welcome panel; without a tab only the explorer.
    pub(super) fn cycle_focus(&mut self, d: i32) {
        // The panes on screen, in order: a table tab has no editor, a query tab shows its
        // results once it ran.
        let order: Vec<Focus> =
            [Focus::Tree, Focus::Editor, Focus::Results].into_iter().filter(|f| self.focusable(*f)).collect();
        let n = order.len() as i32;
        // The inspector counts as the results' place.
        let now = if self.focus == Focus::Inspector { Focus::Results } else { self.focus };
        let i = order.iter().position(|f| *f == now).unwrap_or(0) as i32;
        self.focus = order[((i + d).rem_euclid(n)) as usize];
        self.explorer.filtering = false;
    }

    /// Keys of the completion popup (`overlay.completion`).
    pub(super) fn popup_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let t = self.tabs.active_mut();
        let Some(p) = t.popup.as_mut() else { return };
        let n = p.items.len();
        match key.code {
            KeyCode::Up => p.selected = (p.selected + n - 1) % n,
            KeyCode::Char('p') if ctrl => p.selected = (p.selected + n - 1) % n,
            KeyCode::Down => p.selected = (p.selected + 1) % n,
            KeyCode::Char('n') if ctrl => p.selected = (p.selected + 1) % n,
            KeyCode::Tab | KeyCode::Enter => {
                let label = p.items[p.selected].label.clone();
                let start = p.replace_start;
                let end = t.editor.offset();
                if start <= end {
                    t.editor.replace_range(start, end, &label);
                }
                t.popup = None;
                self.edited();
            }
            KeyCode::Esc => t.popup = None,
            _ => {}
        }
    }

    pub(super) fn update_completion(&mut self, force: bool) {
        // The tab's database's catalog and its search path.
        let (path, other) = (self.tab_path(self.tab()), self.other_database(self.tab()).map(str::to_string));
        if let (Some(p), Some(db)) = (self.tab().profile, other.as_deref()) {
            self.ensure_aux(p, db);
        }
        let catalog = self.conns.catalog_in(self.tab().profile, other.as_deref());
        let t = self.tabs.active_mut();
        // The text around the cursor holds its whole `;`-delimited segment, which is all the
        // completer reads.
        let (base, text, off) = t.editor.completion_context();
        t.popup = complete_in(&text, off, catalog, force, &path).map(|c| Popup {
            items: c.items,
            selected: 0,
            replace_start: base + c.replace_start,
        });
    }

    pub(super) fn ident_prefix_len(&self) -> usize {
        self.tab().editor.ident_before_cursor().chars().count()
    }

    /// A character was typed in Insert mode: refine an open popup at once, otherwise
    /// schedule it (after `.` or once the identifier has `COMPLETION_MIN_PREFIX` chars).
    pub(super) fn on_typed(&mut self, c: char) {
        if self.tab().popup.is_some() {
            self.update_completion(false);
        } else if c == '.' || self.ident_prefix_len() >= COMPLETION_MIN_PREFIX {
            self.tab_mut().completion_due = Some(Instant::now() + COMPLETION_DEBOUNCE);
        } else {
            self.tab_mut().completion_due = None;
        }
    }

    pub(super) fn editor_key(&mut self, key: KeyEvent) {
        // The system clipboard is read only for the command that puts it (`"+p`).
        if self.tab().editor.reads_clipboard(&key) {
            let text = self.editor_clipboard_text();
            self.tab_mut().editor.set_clipboard_text(text.as_deref());
        }
        let t = self.tabs.active_mut();
        let ev = t.editor.handle_key(key);
        let problem = t.editor.take_register_problem();
        if let Some(y) = t.editor.take_yank() {
            self.editor_yanked(y);
        }
        if let Some(p) = problem {
            let msg = match p {
                RegProblem::Empty(c) => Msg::EditorRegisterEmpty { name: format!("\"{c}") },
                RegProblem::ReadOnly(c) => Msg::EditorRegisterReadOnly { name: format!("\"{c}") },
            };
            self.flash(Notice::new(msg, Level::Warning));
        }
        if matches!(ev, EdEvent::Changed { .. }) {
            self.edited();
        }
        let t = self.tabs.active_mut();
        match ev {
            EdEvent::Changed { typed: Some(c) } if is_ident_char(c) || c == '.' => self.on_typed(c),
            EdEvent::Changed { typed: None } if t.popup.is_some() && t.editor.mode == Mode::Insert => {
                t.completion_due = None;
                self.update_completion(false)
            }
            EdEvent::Changed { .. } | EdEvent::Moved => {
                t.popup = None;
                t.completion_due = None;
            }
            EdEvent::None => {}
        }
    }

    /// What a node of profile `id`'s schema tree asked for.
    pub(super) fn tree_action(&mut self, id: ProfileId, action: TreeAction) {
        match action {
            TreeAction::None => {}
            TreeAction::LoadObjects(schema) => self.send_meta(id, DbCommand::LoadObjects { schema }),
            TreeAction::LoadSchemas => {
                self.send_meta(id, DbCommand::LoadSchemas);
                self.send_meta(id, DbCommand::LoadCatalog);
                // The keys are read again too: the cached ones may be out of date.
                self.reload_keys(id);
            }
            TreeAction::Open { schema, name } => self.open_table(id, None, &schema, &name),
            TreeAction::LoadStructure { schema, name } => {
                if !self.structure_on(id) {
                    return;
                }
                self.conns.entry(id).tree.structure_loading(&schema, &name);
                self.send_meta(id, DbCommand::LoadStructure { schema, table: name });
            }
            TreeAction::Reveal { schema, name } => {
                let Some(c) = self.conns.get_mut(id) else { return };
                match c.tree.reveal(&schema, &name) {
                    Reveal::Found(n) => self.explorer.select_kind(RowKind::Node(id, n)),
                    Reveal::Pending(action) => self.tree_action(id, action),
                    Reveal::Missing => self.reveal_missing(&schema, &name),
                }
            }
        }
    }

    /// Read profile `id`'s key columns again (a refresh, or DDL run in the app), when its
    /// driver reports them.
    pub(super) fn reload_keys(&mut self, id: ProfileId) {
        if self.conns.get(id).and_then(|c| c.meta.as_ref()).is_some_and(|s| s.caps.key_metadata) {
            self.conns.entry(id).keys = Keys::Unknown;
            self.send_meta(id, DbCommand::LoadKeys);
        }
    }

    /// Cell viewer keys; `keys` is one key or a whole sequence (`g g`).
    pub(super) fn viewer_keys(&mut self, keys: &[KeyChord]) {
        let Some(v) = self.overlays.viewer_mut() else { return };
        let page = v.view_h.max(1);
        if keys.len() == 2 && keys.iter().all(|k| *k == KeyChord::char('g')) {
            v.scroll = 0;
            return;
        }
        for k in keys {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlays.close(OverlayKind::CellViewer);
                    return;
                }
                KeyCode::Char('j') | KeyCode::Down => v.scroll += 1,
                KeyCode::Char('k') | KeyCode::Up => v.scroll = v.scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => v.scroll += page,
                KeyCode::PageUp => v.scroll = v.scroll.saturating_sub(page),
                KeyCode::Char('G') => v.scroll = usize::MAX / 2,
                _ => {}
            }
        }
    }

    pub(super) fn handle_mouse(&mut self, m: MouseEvent) {
        if self.layout.too_small {
            return;
        }
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let l = self.layout;
        match m.kind {
            // Sideways (a trackpad, a tilting wheel): the grid's columns.
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight if inside(l.results) => {
                let d = if m.kind == MouseEventKind::ScrollRight { 1 } else { -1 };
                self.scroll_grid(0, d);
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d = if m.kind == MouseEventKind::ScrollDown { WHEEL_STEP } else { -WHEEL_STEP };
                if let Some(v) = self.overlays.viewer_mut() {
                    v.scroll = (v.scroll as isize + d).max(0) as usize;
                } else if inside(l.tree) {
                    self.explorer_mouse(m);
                } else if inside(l.editor) {
                    let t = self.tab_mut();
                    t.popup = None;
                    t.editor.scroll(d);
                } else if inside(l.results) && self.tab().exec.view == tabs::ResultView::Messages {
                    let s = &mut self.tab_mut().exec.messages_scroll;
                    *s = (*s as isize + d).max(0) as usize;
                } else if inside(l.results) && m.modifiers.contains(KeyModifiers::SHIFT) {
                    // Shift+wheel: the columns, one a notch.
                    self.scroll_grid(0, d.signum());
                } else if inside(l.results) {
                    self.scroll_grid(d, 0);
                    // A drag goes on: the selection follows the rows that came under the
                    // pointer.
                    if let (Some(Drag::Grid { .. }), Some((dx, dy))) = (self.drag, self.drag_at) {
                        self.drag_to(dx, dy);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right)
                if inside(l.tree) && !self.overlays.is_open(OverlayKind::CellViewer) =>
            {
                self.open_context_menu(x, y);
            }
            MouseEventKind::Down(MouseButton::Right)
                if inside(l.results) && !self.overlays.is_open(OverlayKind::CellViewer) =>
            {
                self.open_grid_menu(x, y);
            }
            MouseEventKind::Down(MouseButton::Left) if inside(l.tab_bar) && !self.tabs.is_empty() => {
                self.tab_bar_click(x, false)
            }
            MouseEventKind::Down(MouseButton::Middle) if inside(l.tab_bar) && !self.tabs.is_empty() => {
                self.tab_bar_click(x, true)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.overlays.is_open(OverlayKind::CellViewer) {
                    return;
                }
                self.left_click(m, inside(l.divider) && l.divider.height > 0);
            }
            MouseEventKind::Drag(MouseButton::Left) => self.drag_to(x, y),
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                self.drag_at = None;
            }
            _ => {}
        }
    }

    /// The wheel over the grid (as DataGrip and a browser): the view scrolls `rows`
    /// rows and `cols` columns within the page; the selected cell stays on its row, also off
    /// screen, until a key or a click moves it. Nothing is fetched (pages are explicit).
    fn scroll_grid(&mut self, rows: isize, cols: isize) {
        let t = self.tabs.active_mut();
        if let Results::Rows(rs) = &t.results {
            let (total, ncols) = (rs.rows.len(), rs.columns.len());
            t.grid.scroll(rows, cols, total, ncols);
        }
    }

    /// A click on the document tab bar at column `x`, by what was drawn there: a
    /// tab becomes active (`close`: a middle click closes it, asking first as `Ctrl+W` does); a
    /// scroll mark activates the nearest hidden tab on its side. The focus stays where it is.
    fn tab_bar_click(&mut self, x: u16, close: bool) {
        use crate::widgets::tabbar::TabHit;
        let Some(hit) = self.tab_hits.iter().find(|(a, b, _)| x >= *a && x < *b).map(|h| h.2) else { return };
        let (index, close) = match hit {
            TabHit::Tab(i) => (i, close),
            TabHit::More(i) if !close => (i, false),
            TabHit::More(_) => return,
            // Its `×`: closed as `Ctrl+W` does, asking first.
            TabHit::Close(i) => (i, true),
        };
        self.overlays.close(OverlayKind::CellViewer);
        self.switch_tab(|m| m.activate(index));
        if close && self.tabs.active_index() == index {
            self.request_close_tab();
        }
    }

    /// A left click at `m` (no cell viewer open). A click on the results pane's top border
    /// (`on_divider`) acts as a click there (a title's tab names, the paging arrows) and starts
    /// a drag that resizes the panes.
    fn left_click(&mut self, m: MouseEvent, on_divider: bool) {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let l = self.layout;
        if inside(l.tree) {
            self.focus = Focus::Tree;
            self.explorer_mouse(m);
            return;
        }
        // The previous and next page marks of the results title.
        if !self.tabs.is_empty() && (inside(l.page_prev) || inside(l.page_next)) {
            self.focus = Focus::Results;
            return if inside(l.page_prev) { self.page_prev() } else { self.page_next() };
        }
        // A second click on the same cell soon after the first: the cell viewer (a word
        // in the editor; a third click there: the line).
        let again = self.last_click.is_some_and(|(t, lx, ly)| t.elapsed() < DOUBLE_CLICK && (lx, ly) == (x, y));
        self.clicks = if again { (self.clicks + 1).min(3) } else { 1 };
        let double = self.clicks == 2;
        self.last_click = Some((Instant::now(), x, y));
        if self.profiles.is_empty() {
            if inside(l.editor) {
                self.focus = Focus::Editor;
            }
        } else if self.tabs.is_empty() {
            // The empty state: nothing to focus there.
        } else if inside(l.editor) {
            self.focus = Focus::Editor;
            let i = l.editor_text;
            let clicks = self.clicks;
            let t = self.tab_mut();
            t.popup = None;
            if inside(i) {
                let (cx, cy) = (x - i.x, y - i.y);
                match clicks {
                    2 => t.editor.select_word(t.editor.pos_at(cx, i32::from(cy))),
                    3 => t.editor.select_line(t.editor.pos_at(cx, i32::from(cy)).0),
                    _ => {
                        t.editor.click(cx, cy);
                        let anchor = (t.editor.row, t.editor.col);
                        self.drag = Some(Drag::Editor { anchor });
                    }
                }
            }
        } else if inside(l.strip) {
            // A result tab of the strip.
            self.focus = Focus::Results;
            let hit = self.strip_hits.iter().find(|(a, b, _)| x >= *a && x < *b).map(|h| h.2);
            match hit {
                Some(Some(i)) => self.tab_mut().show_result(i),
                Some(None) => self.tab_mut().exec.view = tabs::ResultView::Messages,
                None => {}
            }
        } else if inside(l.results) {
            self.focus = Focus::Results;
            let t = self.tabs.active_mut();
            // The row-number gutter selects whole rows, a header whole columns;
            // Shift extends the selection there is; a drag from there extends it.
            let extend = m.modifiers.contains(KeyModifiers::SHIFT);
            let whole = match &t.results {
                Results::Rows(rs) => {
                    let g = &t.grid;
                    let row = g.gutter_row(x, y, rs.rows.len()).map(|r| (Shape::Rows, (r, g.col)));
                    row.or_else(|| g.header_col(x, y).map(|c| (Shape::Cols, (g.row, c))))
                }
                _ => None,
            };
            if let Some((shape, to)) = whole {
                t.grid.select_whole(shape, to, extend);
                let anchor = t.grid.anchor.unwrap_or(to);
                self.drag = Some(Drag::Grid { anchor, shape });
                return;
            }
            let hit = match &t.results {
                Results::Rows(rs) => t.grid.click(x, y, rs.rows.len()),
                _ => false,
            };
            if hit && double {
                self.last_click = None;
                self.grid_action(GridAction::ViewCell);
            } else if hit {
                // A click drops the selected range; a drag from here starts one.
                t.grid.anchor = None;
                self.drag = Some(Drag::Grid { anchor: (t.grid.row, t.grid.col), shape: Shape::Cells });
            }
        } else if inside(l.detail) {
            // The inspector takes the focus; its title's tab names switch tabs.
            self.focus = Focus::Inspector;
            if y == l.detail.y {
                let rel = x.saturating_sub(l.detail.x);
                let tabs = crate::widgets::inspector::tab_labels(&self.i18n, self.detail.tab);
                if (tabs[0].1..tabs[0].2).contains(&rel) {
                    self.detail.tab = DetailTab::Cell;
                } else if (tabs[1].1..tabs[1].2).contains(&rel) {
                    self.detail.tab = DetailTab::Row;
                }
            }
        }
        if on_divider {
            self.drag = Some(Drag::Divider);
        }
    }

    /// The left button moved to (x, y) while held: extend the drag's selection. Past the
    /// editor's top or bottom the selection goes one line further, so the view scrolls.
    fn drag_to(&mut self, x: u16, y: u16) {
        match self.drag {
            Some(Drag::Editor { anchor }) if self.focus == Focus::Editor && !self.tabs.is_empty() => {
                let i = self.layout.editor_text;
                let rel_y = (i32::from(y) - i32::from(i.y)).clamp(-1, i32::from(i.height));
                let rel_x = x.saturating_sub(i.x).min(i.width.saturating_sub(1));
                let t = self.tab_mut();
                let to = t.editor.pos_at(rel_x, rel_y);
                t.editor.drag_select(anchor, to);
            }
            Some(Drag::Divider) if !self.tabs.is_empty() => self.drag_divider(y),
            Some(Drag::Grid { anchor, shape }) if self.focus == Focus::Results && !self.tabs.is_empty() => {
                self.drag_at = Some((x, y));
                let t = self.tabs.active_mut();
                let Results::Rows(rs) = &t.results else { return };
                let (total, cols) = (rs.rows.len(), rs.columns.len());
                let (row, col) = t.grid.drag_target(x, y, total, cols);
                // Whole rows follow the pointer's row only, whole columns its column only.
                let to = match shape {
                    Shape::Cells => (row, col),
                    Shape::Rows => (row, t.grid.col),
                    Shape::Cols => (t.grid.row, col),
                };
                (t.grid.row, t.grid.col) = to;
                t.grid.detached = false;
                // A rectangle as `v` makes it, once the pointer left the first cell.
                if shape != Shape::Cells || to != anchor || t.grid.anchor.is_some() {
                    t.grid.anchor = Some(anchor);
                    t.grid.shape = shape;
                }
            }
            _ => {}
        }
    }
}
