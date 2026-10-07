//! Work the binary does outside the app, with the terminal handed over: the query in the
//! user's own editor (`Ctrl+G`) and suspending (`Ctrl+Z`, `:suspend`). The app queues an
//! [`Effect`]; the binary runs it between frames and reports what came back
//! ([`App::external_edit_done`]). Meanwhile nothing else runs in the app: a query keeps
//! running, and what arrives for it is applied once the app runs again.
//!
//! The editor's text replaces the tab's as one undo step (`u` undoes it). A table tab's query
//! and a DDL tab's text are not texts anybody wrote: the editor gets a copy, and an edited copy
//! opens in a new console on the same connection, database and schema.

use super::*;
use crate::external::{EditFailure, Edited, Ended};
use datarig_core::driver::SessionContext;
use std::path::PathBuf;

/// What the binary does for the app, with the terminal handed over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Edit `text` in the user's editor, in a new file under `dir` (`None`: there is no state
    /// directory), then call [`App::external_edit_done`].
    Edit { text: String, dir: Option<PathBuf> },
    /// Stop until the shell continues the program (`fg`), then redraw everything; a failure
    /// goes to [`App::suspend_failed`].
    Suspend,
}

/// The tab an external edit is for, as it was when the editor started.
#[derive(Clone, Debug)]
pub(super) struct PendingEdit {
    tab: TabId,
    version: u64,
    /// A table tab: the result goes to a new console.
    copy: bool,
    profile: Option<ProfileId>,
    context: SessionContext,
}

/// Suspending is a Unix job-control feature.
pub const SUSPEND_SUPPORTED: bool = cfg!(unix);

impl App {
    /// What the binary is to do now, once.
    pub fn take_effect(&mut self) -> Option<Effect> {
        self.effect.take()
    }

    /// `Ctrl+G`: the active tab's text in the user's editor. The editor goes back to Normal
    /// mode first (as `Esc` does: Insert ends, a selection or a pending command goes).
    pub(super) fn request_external_edit(&mut self) {
        if self.effect.is_some() {
            return;
        }
        // A table tab's query and a DDL tab's text are not the tab's to change: a copy, which
        // goes to a new console.
        let copy = !self.tab().is_query();
        if !copy {
            for _ in 0..3 {
                let e = &self.tab().editor;
                if e.mode == Mode::Normal && !e.awaiting_key() && self.key_context() != Ctx::VimSearch {
                    break;
                }
                self.editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            }
        }
        let t = self.tab_mut();
        t.popup = None;
        t.completion_due = None;
        let t = self.tab();
        let pending = PendingEdit {
            tab: t.id,
            version: t.editor.version(),
            copy,
            profile: t.profile,
            context: t.context.clone(),
        };
        let text = t.editor.text();
        self.editing = Some(pending);
        let dir = self.paths.state.as_ref().map(|s| s.join("edit"));
        self.effect = Some(Effect::Edit { text, dir });
    }

    /// What came back from the editor [`Effect::Edit`] ran: new text replaces the tab's (one
    /// undo step) or, for a table tab, opens in a new console; anything else is said and the
    /// text stays. A tab that is gone or changed meanwhile keeps its text, and the edited
    /// text opens in a new console (nothing typed in the editor is lost).
    pub fn external_edit_done(&mut self, edited: Edited) {
        let Some(p) = self.editing.take() else { return };
        let text = match edited {
            Edited::Changed(text) => text,
            Edited::Unchanged => return self.flash(Notice::new(Label::ExternalUnchanged, Level::Info)),
            Edited::Failed(f) => return self.flash(Notice::new(failure(f), Level::Error)),
        };
        let current = !self.tabs.is_empty() && self.tab().id == p.tab && self.tab().editor.version() == p.version;
        if p.copy || !current {
            self.console_with(&text, p.profile, p.context);
            let label = if p.copy { Label::ExternalCopied } else { Label::ExternalStale };
            let level = if p.copy { Level::Info } else { Level::Warning };
            return self.flash(Notice::new(label, level));
        }
        let t = self.tab_mut();
        t.editor.replace_text(&text);
        t.popup = None;
        self.edited();
        self.flash(Notice::new(Label::ExternalReplaced, Level::Info));
    }

    /// A new console after the active tab, holding `text`, on `profile` in `context` (the
    /// profile connects when it is not connected); the editor gets the focus.
    pub(super) fn console_with(&mut self, text: &str, profile: Option<ProfileId>, context: SessionContext) {
        match profile {
            Some(id) => self.console_in(id, context),
            None => {
                self.leave_tab();
                let tab = self.tabs.open(TabKind::Console, None, Editor::new(""));
                self.sync_tab_language(tab);
                self.entered_tab();
                self.focus = Focus::Editor;
            }
        }
        self.tab_mut().editor = Editor::new(text);
        self.edited();
    }

    /// `Ctrl+Z`, `:suspend`: stop until the shell's `fg` (Unix only).
    pub(super) fn request_suspend(&mut self) {
        if !SUSPEND_SUPPORTED {
            return self.flash(Notice::new(Label::SuspendUnsupported, Level::Error));
        }
        if self.effect.is_none() {
            self.effect = Some(Effect::Suspend);
        }
    }

    /// The program could not stop itself (`error`); it goes on.
    pub fn suspend_failed(&mut self, error: String) {
        self.flash(Notice::new(Msg::SuspendFailed { error }, Level::Error));
    }
}

/// What to say about an edit that brought nothing back.
fn failure(f: EditFailure) -> Msg {
    match f {
        EditFailure::NoStateDir => Msg::Label(Label::ExternalNoStateDir),
        EditFailure::File(error) => Msg::ExternalFile { error },
        EditFailure::Terminal(error) => Msg::ExternalTerminal { error },
        EditFailure::Command { var, error } => Msg::ExternalCommand { var: var.to_string(), error },
        EditFailure::Spawn { program, error } => Msg::ExternalSpawn { program, error },
        EditFailure::Exit { program, how: Ended::Code(code) } => {
            Msg::ExternalExitCode { program, code: code.to_string() }
        }
        EditFailure::Exit { program, how: Ended::Signal(signal) } => {
            Msg::ExternalExitSignal { program, signal: signal.to_string() }
        }
        EditFailure::Exit { program, how: Ended::Other(how) } => Msg::ExternalExitOther { program, how },
        EditFailure::Read(error) => Msg::ExternalRead { error },
        EditFailure::NotUtf8 => Msg::Label(Label::ExternalNotUtf8),
    }
}
