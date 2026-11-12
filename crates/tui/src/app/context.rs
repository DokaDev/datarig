//! The key context: which part of the app keys go to right now.

use super::*;

impl App {
    /// The key context: the top dialog, else the focused pane.
    pub fn key_context(&self) -> Ctx {
        self.context_of(self.overlays.top())
    }

    /// The context under the command line (whose keys its list shows).
    pub fn context_below_commands(&self) -> Ctx {
        self.context_of(self.overlays.iter().rev().find(|o| o.kind() != OverlayKind::Commands))
    }

    fn context_of(&self, top: Option<&Overlay>) -> Ctx {
        // The status bar's question takes every key before what is under it (not a dialog's).
        if self.ask.is_some() && !self.modal_open() {
            return Ctx::StatusAsk;
        }
        match top {
            Some(Overlay::Commands(_)) => Ctx::Commands,
            Some(Overlay::Password(_)) => Ctx::Password,
            Some(Overlay::CellViewer(_)) => Ctx::CellViewer,
            Some(Overlay::WhichKey(_)) => Ctx::WhichKey,
            // Only quitting (and the root keys) while it waits.
            Some(Overlay::Busy(_)) => Ctx::Busy,
            Some(Overlay::Confirm(_)) => Ctx::Confirm,
            Some(Overlay::RunConfirm(_)) => Ctx::RunConfirm,
            Some(Overlay::TxSummary(_)) => Ctx::TxSummary,
            Some(Overlay::EditPreview(_)) => Ctx::EditPreview,
            Some(Overlay::SessionView(_)) => Ctx::SessionView,
            Some(Overlay::WhatsNew(_)) => Ctx::WhatsNew,
            Some(Overlay::UpdateNotice(_)) => Ctx::UpdateNotice,
            Some(Overlay::IconsAsk(_)) => Ctx::IconsAsk,
            Some(Overlay::Help(h)) if h.filtering => Ctx::HelpFilter,
            Some(Overlay::Help(_)) => Ctx::Help,
            Some(Overlay::ProfileForm(_)) => Ctx::ProfileForm,
            Some(Overlay::Settings(_)) => Ctx::Settings,
            Some(Overlay::Export(_)) => Ctx::Export,
            Some(Overlay::FilterDialog(d)) if d.typing() => Ctx::FilterDialogText,
            Some(Overlay::FilterDialog(_)) => Ctx::FilterDialog,
            Some(Overlay::QuickConnect(_)) => Ctx::QuickConnect,
            Some(Overlay::TabList(_)) => Ctx::TabList,
            Some(Overlay::NameInput(_)) => Ctx::NameInput,
            Some(Overlay::ScriptTree(t)) if t.focus == script_tree::TreeFocus::Name => Ctx::ScriptTreeName,
            Some(Overlay::ScriptTree(_)) => Ctx::ScriptTree,
            Some(Overlay::ContextMenu(_)) => Ctx::ContextMenu,
            Some(Overlay::Chooser(c)) if c.filtering => Ctx::ChooserFilter,
            Some(Overlay::Chooser(_)) => Ctx::Chooser,
            Some(Overlay::Picker(_)) => Ctx::Picker,
            // Too small to draw the panes: only the workspace keys (run, new tab, …).
            None if self.layout.too_small => Ctx::Workspace,
            None => match self.focus {
                Focus::Tree if self.explorer.filtering => Ctx::ExplorerFilter,
                Focus::Tree => Ctx::Explorer,
                // Without profiles the welcome panel takes the place of the editor and results.
                _ if self.profiles.is_empty() => Ctx::Welcome,
                // No tab: the empty state has no keys of its own.
                _ if self.tabs.is_empty() => Ctx::Explorer,
                Focus::Results | Focus::Inspector if self.monitor_filtering() => Ctx::MonitorFilter,
                Focus::Results | Focus::Inspector if self.monitor_shown() => Ctx::Monitor,
                Focus::Results | Focus::Inspector if self.plan_shown() => Ctx::Plan,
                Focus::Results | Focus::Inspector if self.chart_shown() => Ctx::Chart,
                Focus::Results | Focus::Inspector if self.changes_shown() => Ctx::Changes,
                Focus::Results if self.cell_lines_open() => Ctx::CellEditor,
                Focus::Results if self.cell_edit_open() => Ctx::CellEdit,
                Focus::Results if self.grid_prompt_open() => Ctx::GridSearch,
                Focus::Results if self.grid_search_on() => Ctx::GridMatches,
                Focus::Results => Ctx::Grid,
                Focus::Inspector if self.inspector_shown() => Ctx::Inspector,
                Focus::Inspector => Ctx::Grid,
                Focus::Editor if self.tab().editor.searching() => Ctx::VimSearch,
                // A DDL tab's own keys, unless a vim command is being typed (a count, `f`, `m`).
                Focus::Editor
                    if self.tab().is_ddl()
                        && self.tab().editor.mode == Mode::Normal
                        && !self.tab().editor.command_started() =>
                {
                    Ctx::Ddl
                }
                Focus::Editor => match self.tab().editor.mode {
                    Mode::Normal => Ctx::VimNormal,
                    Mode::Insert => Ctx::VimInsert,
                    Mode::Visual => Ctx::VimVisual,
                },
            },
        }
    }
}
