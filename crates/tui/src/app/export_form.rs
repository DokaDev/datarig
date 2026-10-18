//! The export dialog (`results.export`, `Space r e`, the results menu, `:export` alone): what of
//! the active tab's result to export (every row, the rows fetched, the selection while there is
//! one), the format, CSV's options (TSV's header line), and the file; then Export or Cancel.
//! Under the rows and the format it says, as they change, how the rows will be read (from the
//! result; by the table's key in chunks; with the statement run again once, holding its read
//! lock and snapshot; inside the user's transaction) and what a format cannot do here (an INSERT
//! or UPDATE the result's keys do not allow, as a copy says it; a format that comes in a later
//! version, listed greyed). `Esc` or Cancel closes it.
//!
//! Export hands the request to [`App::start_export`], which asks what it asks (a file in the
//! way, a statement run again, a read-only or prod-like environment). The dialog is gone by
//! then: the export runs in the background, its progress in the tab's status.
//!
//! The path starts as `~/Downloads/<table or tab>-<YYYYMMDD-HHMMSS>.<ext>`; while its file name
//! is that, a format change gives it the new format's extension. `~` is the home folder, a
//! relative path goes in `~/Downloads`. Anything that fills the path field from outside goes
//! through [`ExportForm::set_path`].

use super::copy::{CopyWhat, Intent};
use super::export::{ExportFormat, ExportRequest, ExportScope, LATER_FORMATS, expand_path};
use super::overlay::Buttons;
use super::picker::Picker;
use super::*;
use crate::widgets::list_picker::{Entry, ListPicker, Outcome};
use crate::widgets::text_input::InputResult;
use datarig_core::driver::ReadIn;
use datarig_core::export::stream::{BadDelimited, Delimited, Quote};
use ratatui::layout::Position;

/// A field of the export dialog, or one of its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportField {
    Scope,
    Format,
    Delimiter,
    Quote,
    Header,
    Null,
    Path,
    Export,
    Cancel,
}

impl ExportField {
    pub fn label(self) -> Label {
        match self {
            ExportField::Scope => Label::ExportFieldScope,
            ExportField::Format => Label::ExportFieldFormat,
            ExportField::Delimiter => Label::ExportFieldDelimiter,
            ExportField::Quote => Label::ExportFieldQuote,
            ExportField::Header => Label::ExportFieldHeader,
            ExportField::Null => Label::ExportFieldNull,
            ExportField::Path => Label::ExportFieldPath,
            ExportField::Export => Label::DialogButtonExport,
            ExportField::Cancel => Label::DialogButtonCancel,
        }
    }

    /// A value picked from a list (`Enter`), or stepped through (`←`/`→`).
    pub fn is_choice(self) -> bool {
        matches!(self, ExportField::Scope | ExportField::Format | ExportField::Quote)
    }

    /// Text typed in.
    pub fn is_text(self) -> bool {
        matches!(self, ExportField::Delimiter | ExportField::Null | ExportField::Path)
    }

    pub fn is_button(self) -> bool {
        matches!(self, ExportField::Export | ExportField::Cancel)
    }
}

/// Why Export did not start, and the field to change.
type Refusal = Box<(ExportField, Notice)>;

/// An entry of the format list: a format the app writes, or one it lists greyed until it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatChoice {
    Ready(ExportFormat),
    Later(Label),
}

/// The format list, in order: every [`ExportFormat`], the formats of [`LATER_FORMATS`] still to
/// come after the delimited ones and Excel.
pub fn format_choices() -> Vec<FormatChoice> {
    let mut out = Vec::new();
    for f in ExportFormat::ALL {
        out.push(FormatChoice::Ready(f));
        if f == ExportFormat::Xlsx {
            let later = LATER_FORMATS.iter().filter(|(name, _)| ExportFormat::parse(name).is_none());
            out.extend(later.map(|&(_, label)| FormatChoice::Later(label)));
        }
    }
    out
}

/// CSV's quoting, in the order the list shows it.
pub const QUOTES: [Quote; 4] = [Quote::Necessary, Quote::Always, Quote::NonNumeric, Quote::Never];

pub fn quote_label(q: Quote) -> Label {
    match q {
        Quote::Necessary => Label::ExportQuoteNecessary,
        Quote::Always => Label::ExportQuoteAlways,
        Quote::NonNumeric => Label::ExportQuoteNonNumeric,
        Quote::Never => Label::ExportQuoteNever,
    }
}

fn quote_hint(q: Quote) -> Label {
    match q {
        Quote::Necessary => Label::ExportQuoteNecessaryHint,
        Quote::Always => Label::ExportQuoteAlwaysHint,
        Quote::NonNumeric => Label::ExportQuoteNonNumericHint,
        Quote::Never => Label::ExportQuoteNeverHint,
    }
}

pub fn scope_label(s: ExportScope) -> Label {
    match s {
        ExportScope::All => Label::ExportScopeAll,
        ExportScope::Fetched => Label::ExportScopeFetched,
        ExportScope::Selection => Label::ExportScopeSelection,
    }
}

fn scope_hint(s: ExportScope) -> Label {
    match s {
        ExportScope::All => Label::ExportScopeAllHint,
        ExportScope::Fetched => Label::ExportScopeFetchedHint,
        ExportScope::Selection => Label::ExportScopeSelectionHint,
    }
}

/// What a click on the dialog hits, as its renderer drew it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportHit {
    /// A field's line: it takes the focus.
    Field(ExportField),
    /// A value to pick from a list, or the header's checkbox.
    Value(ExportField),
    /// A text input: the cursor goes under the pointer.
    Input(ExportField),
}

pub struct ExportForm {
    /// The tab whose result it exports.
    pub tab: TabId,
    /// The result it was opened on, on that tab's session, and the selected range then: the
    /// dialog closes when either changes ([`App::sweep_export_dialog`]).
    intent: Intent,
    result: u64,
    selection: Option<(std::ops::Range<usize>, Vec<usize>)>,
    /// The tab's or the table's name (the title).
    pub name: String,
    /// The scopes offered: the selection only while there is one.
    pub scopes: Vec<ExportScope>,
    pub scope: ExportScope,
    pub format: ExportFormat,
    pub quote: Quote,
    pub header: bool,
    pub delimiter: TextInput,
    pub null: TextInput,
    pub path: TextInput,
    /// The folder of the path it started with, as typed (`~/Downloads/`), and the parts of the
    /// file name made for it: while the path is that folder and that name, it is the default
    /// one, and a format change makes the name again.
    folder: String,
    stem: String,
    stamp: String,
    pub focus: ExportField,
    /// Why Export did not start, at the field to change.
    pub error: Option<(ExportField, Notice)>,
    /// Where the renderer drew what can be clicked (most specific last).
    pub hits: Vec<(Rect, ExportHit)>,
    pub buttons: Buttons,
}

impl ExportForm {
    /// The file name made for format `f`.
    fn made_name(&self, f: ExportFormat) -> String {
        format!("{}-{}.{}", self.stem, self.stamp, f.extension())
    }

    /// The default path of format `f`, as the field shows it.
    pub fn default_text(&self, f: ExportFormat) -> String {
        format!("{}{}", self.folder, self.made_name(f))
    }

    /// The fields shown, in order, the buttons last.
    pub fn fields(&self) -> Vec<ExportField> {
        use ExportField::*;
        let mut v = vec![Scope, Format];
        match self.format {
            ExportFormat::Csv => v.extend([Delimiter, Quote, Header, Null]),
            ExportFormat::Tsv => v.push(Header),
            // An empty cell, or this text.
            ExportFormat::Xlsx => v.push(Null),
            _ => {}
        }
        v.extend([Path, Export, Cancel]);
        v
    }

    /// The focus `d` fields on (around the ends).
    fn step_focus(&mut self, d: isize) {
        let fields = self.fields();
        let i = fields.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(i + d).rem_euclid(fields.len() as isize) as usize];
    }

    pub fn input_mut(&mut self, f: ExportField) -> Option<&mut TextInput> {
        match f {
            ExportField::Delimiter => Some(&mut self.delimiter),
            ExportField::Null => Some(&mut self.null),
            ExportField::Path => Some(&mut self.path),
            _ => None,
        }
    }

    /// Put `text` in the path field, the cursor at its end.
    pub fn set_path(&mut self, text: &str) {
        self.path.set(text);
        self.clear_error(ExportField::Path);
    }

    /// Format `f` from now on. A path whose file name is still the one made for the format
    /// before (its folder may have been changed) gets the one made for `f`.
    pub fn set_format(&mut self, f: ExportFormat) {
        let made = self.made_name(self.format);
        if let Some(folder) = self.path.text().strip_suffix(made.as_str())
            && (folder.is_empty() || folder.ends_with(['/', '\\']))
        {
            let text = format!("{folder}{}", self.made_name(f));
            self.path.set(&text);
        }
        self.format = f;
        self.clear_error(ExportField::Format);
        // What was said at a field the format does not show is not said again later.
        if self.error.as_ref().is_some_and(|(at, _)| !self.fields().contains(at)) {
            self.error = None;
        }
    }

    fn clear_error(&mut self, f: ExportField) {
        if self.error.as_ref().is_some_and(|(at, _)| *at == f) {
            self.error = None;
        }
    }

    /// CSV's options as the fields have them (TSV's header line; the defaults for the other
    /// formats), or why they cannot be written.
    fn delimited(&self) -> Result<Delimited, Refusal> {
        let mut d = Delimited { header: self.header, ..Delimited::default() };
        if self.format != ExportFormat::Csv {
            return Ok(d);
        }
        let mut chars = self.delimiter.text().chars();
        d.delimiter = match (self.delimiter.text(), chars.next(), chars.next()) {
            ("\\t", _, _) => '\t',
            (_, Some(c), None) => c,
            _ => {
                let n = Notice::new(Label::ExportDialogBadDelimiter, Level::Error);
                return Err(Box::new((ExportField::Delimiter, n)));
            }
        };
        d.quote = self.quote;
        d.null = self.null.text().to_string();
        d.check().map_err(|e| match e {
            BadDelimited::Delimiter => {
                Box::new((ExportField::Delimiter, Notice::new(Label::ExportBadDelimiter, Level::Error)))
            }
            BadDelimited::Null => Box::new((ExportField::Null, Notice::new(Label::ExportBadNull, Level::Error))),
        })?;
        Ok(d)
    }
}

impl App {
    /// Open the export dialog on the active tab's result.
    pub(super) fn open_export_dialog(&mut self) {
        if self.tabs.is_empty() || self.layout.too_small {
            return;
        }
        let id = self.tab().id;
        if self.exporting(id) {
            return self.flash(Notice::new(Label::ExportBusyTab, Level::Warning));
        }
        let Some(every) = self.block(CopyWhat::Every) else {
            return self.flash(Notice::new(Label::ExportNothing, Level::Warning));
        };
        let t = self.tab();
        let name = match &t.doc.table {
            Some(table) => table.name.clone(),
            None => crate::widgets::tabbar::document_name(self, t),
        };
        let (intent, result) = (Intent::of(t), self.export_result_id());
        let selection = self.export_selection();
        let scopes: Vec<ExportScope> =
            ExportScope::ALL.into_iter().filter(|s| *s != ExportScope::Selection || selection.is_some()).collect();
        // `/` on every system (Windows takes it too): the path reads the same everywhere.
        let folder = match self.home() {
            Some(_) => "~/Downloads/".to_string(),
            None => String::new(),
        };
        let mut form = ExportForm {
            tab: id,
            intent,
            result,
            selection,
            name,
            scopes,
            scope: ExportScope::All,
            format: ExportFormat::Csv,
            quote: Quote::Necessary,
            header: true,
            delimiter: TextInput::new(","),
            null: TextInput::default(),
            path: TextInput::default(),
            folder,
            stem: self.export_stem(),
            stamp: chrono::Local::now().format("%Y%m%d-%H%M%S").to_string(),
            focus: ExportField::Scope,
            error: None,
            hits: Vec::new(),
            buttons: Buttons::default(),
        };
        let text = form.default_text(ExportFormat::Csv);
        form.path.set(&text);
        let t = self.tab_mut();
        t.popup = None;
        t.completion_due = None;
        self.overlays.close(OverlayKind::WhichKey);
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::Export(Box::new(form)));
        if every.partial {
            self.export_ask_key(id);
        }
    }

    /// The dialog opened on a table tab whose every row may be read by the table's key: the
    /// table's structure is asked for (once), so the dialog can say whether it is.
    fn export_ask_key(&mut self, id: TabId) {
        let Some((pid, table)) = self.export_keyset_table(id) else { return };
        let tree = &mut self.conns.entry(pid).tree;
        if tree.structures.contains_key(&(table.schema.clone(), table.name.clone())) {
            return;
        }
        tree.structure_loading(&table.schema, &table.name);
        self.send_meta(pid, DbCommand::LoadStructure { schema: table.schema, table: table.name });
    }

    fn close_export_dialog(&mut self) {
        self.overlays.close(OverlayKind::Export);
        if matches!(self.overlays.picker(), Some(Picker::Export { .. })) {
            self.overlays.close(OverlayKind::Picker);
        }
    }

    /// The id of the active tab's result shown.
    fn export_result_id(&self) -> u64 {
        match &self.tab().results {
            Results::Rows(rs) => rs.id,
            _ => 0,
        }
    }

    /// The active tab's selected range (rows, columns), while there is one.
    fn export_selection(&self) -> Option<(std::ops::Range<usize>, Vec<usize>)> {
        self.tab().grid.anchor?;
        self.block(CopyWhat::Cell).map(|b| (b.rows, b.cols))
    }

    /// The dialog still shows what it was opened on: its tab is the active one, with the same
    /// result on the same session, and the same selection.
    fn export_form_holds(&self, f: &ExportForm) -> bool {
        !self.tabs.is_empty()
            && self.tab().id == f.tab
            && f.intent.holds(self)
            && self.export_result_id() == f.result
            && (f.selection.is_none() || self.export_selection() == f.selection)
    }

    /// After every event: a dialog whose result, selection or tab changed under it (a statement
    /// that answered, another tab made active over it) closes and says so; it never exports
    /// what it did not show.
    pub(super) fn sweep_export_dialog(&mut self) {
        if self.overlays.export_form().is_some_and(|f| !self.export_form_holds(f)) {
            self.close_export_dialog();
            self.flash(Notice::new(Label::ExportStale, Level::Warning));
        }
    }

    /// The dialog's scope notes: how its rows will be read.
    pub(crate) fn export_scope_notes(&self, f: &ExportForm) -> Vec<Notice> {
        if self.tabs.is_empty() || self.tab().id != f.tab {
            return Vec::new();
        }
        let what = if f.scope == ExportScope::Selection { CopyWhat::Cell } else { CopyWhat::Every };
        let Some(b) = self.block(what) else { return vec![Notice::new(Label::ExportNothing, Level::Warning)] };
        let count = b.rows.len() as u64;
        let m = match f.scope {
            ExportScope::Selection => Msg::ExportNoteSelection { count },
            ExportScope::Fetched if b.partial => Msg::ExportNoteFetched { count },
            ExportScope::All if b.partial => return self.export_server_notes(f.tab),
            ExportScope::Fetched | ExportScope::All => Msg::ExportNoteEvery { count },
        };
        vec![Notice::new(m, Level::Info)]
    }

    /// How every row of tab `id`'s result will be read again on its session: by the table's
    /// key, with the statement once (and whether that asks first), inside the user's block; or
    /// why it cannot be.
    fn export_server_notes(&self, id: TabId) -> Vec<Notice> {
        if let Err(n) = self.export_db_checks(id) {
            return vec![n];
        }
        let Some(t) = self.tabs.get(id) else { return Vec::new() };
        let Some(pid) = t.profile else { return Vec::new() };
        let mut estimate = self.table_estimate(t);
        if let Some((_, table)) = self.export_keyset_table(id) {
            let key = (table.schema.clone(), table.name.clone());
            let view = self.conns.get(pid).and_then(|c| c.tree.structures.get(&key));
            match view.map(|v| &v.structure) {
                Some(crate::widgets::tree::Structure::Loaded(s)) => {
                    estimate = estimate.or(s.estimated_rows);
                    if self.export_keyset(id, &table.schema, &table.name, s).is_some() {
                        let table = format!("{}.{}", table.schema, table.name);
                        let mut out = vec![Notice::new(Msg::ExportNoteKeyset { table }, Level::Info)];
                        out.extend(
                            estimate.map(|count| Notice::new(Msg::ExportConfirmEstimate { count }, Level::Info)),
                        );
                        return out;
                    }
                }
                Some(crate::widgets::tree::Structure::Failed(_)) => {}
                Some(crate::widgets::tree::Structure::Loading) | None => {
                    return vec![Notice::new(Label::ExportReadingKey, Level::Info)];
                }
            }
        }
        // One statement (in the user's block: there): whether it asks first, as the export does.
        let read = if t.exec.in_block { Label::ExportNoteInTx } else { Label::ExportNoteStatement };
        let mut out = vec![Notice::new(read, Level::Warning)];
        out.extend(estimate.map(|count| Notice::new(Msg::ExportConfirmEstimate { count }, Level::Info)));
        let (rerun, environment) = self.export_asks(id, t.shown_sql().trim());
        out.extend(rerun.map(|why| Notice::new(Msg::ExportNoteRerun { why }, Level::Warning)));
        out.extend(
            environment.map(|environment| Notice::new(Msg::ExportNoteEnvironment { environment }, Level::Warning)),
        );
        out
    }

    /// Why `format` cannot export `scope` of the active tab's result: an INSERT or UPDATE the
    /// result's keys and values do not allow, with the copy's reason.
    pub(crate) fn export_refusal(&self, scope: ExportScope, format: ExportFormat) -> Option<Notice> {
        let reason = self.export_refusal_reason(scope, format)?;
        let format = self.i18n.label(format.dialog_label()).to_string();
        Some(Notice::new(Msg::ExportNoteSqlRefused { format, reason }, Level::Warning))
    }

    /// [`App::export_refusal`]'s reason, as a copy words it.
    fn export_refusal_reason(&self, scope: ExportScope, format: ExportFormat) -> Option<String> {
        if !matches!(format, ExportFormat::SqlInsert | ExportFormat::SqlUpdate) || self.tabs.is_empty() {
            return None;
        }
        let what = if scope == ExportScope::Selection { CopyWhat::Cell } else { CopyWhat::Every };
        let b = self.block(what)?;
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return None };
        let db = scope == ExportScope::All && b.partial;
        let read_in = if db { ReadIn::first(t.exec.values.as_ref()) } else { rs.read_in.clone() };
        let req = ExportRequest {
            scope,
            format,
            csv: Delimited::default(),
            path: PathBuf::new(),
            default_path: false,
            overwrite: false,
        };
        let refused = self.export_refused(&b.cols, &req, &read_in)?;
        Some(match refused.msg {
            Msg::CopyInsertRefused { reason } | Msg::CopyUpdateRefused { reason } => reason,
            Msg::CopySqlNotWhole { column, reason } => format!("{column}: {reason}"),
            m => self.i18n.msg(&m).to_string(),
        })
    }

    /// The entries of choice field `field` of the dialog (each value's index in its list), and
    /// the current one's.
    fn export_choices(&self, f: &ExportForm, field: ExportField) -> (Vec<Entry<usize>>, Option<usize>) {
        let i18n = &self.i18n;
        let entry = |i: usize, label: Label, hint: String| Entry::new(i, i18n.label(label).to_string(), hint);
        match field {
            ExportField::Scope => {
                let entries = f
                    .scopes
                    .iter()
                    .enumerate()
                    .map(|(i, s)| entry(i, scope_label(*s), i18n.label(scope_hint(*s)).to_string()))
                    .collect();
                (entries, f.scopes.iter().position(|s| *s == f.scope))
            }
            ExportField::Format => {
                let choices = format_choices();
                let entries = choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| match c {
                        FormatChoice::Ready(fmt) => {
                            let refused = self.export_refusal_reason(f.scope, *fmt);
                            let mut e = entry(
                                i,
                                fmt.dialog_label(),
                                refused.clone().unwrap_or_else(|| format!(".{}", fmt.extension())),
                            );
                            e.disabled = refused.is_some();
                            e
                        }
                        FormatChoice::Later(label) => {
                            let mut e = entry(i, *label, i18n.label(Label::ExportFormatLater).to_string());
                            e.disabled = true;
                            e
                        }
                    })
                    .collect();
                (entries, choices.iter().position(|c| *c == FormatChoice::Ready(f.format)))
            }
            ExportField::Quote => {
                let entries = QUOTES
                    .iter()
                    .enumerate()
                    .map(|(i, q)| entry(i, quote_label(*q), i18n.label(quote_hint(*q)).to_string()))
                    .collect();
                (entries, QUOTES.iter().position(|q| *q == f.quote))
            }
            _ => (Vec::new(), None),
        }
    }

    /// `Enter` on a choice field: the list of what it can be, the current value selected.
    fn open_export_picker(&mut self, field: ExportField) {
        let Some(f) = self.overlays.export_form() else { return };
        let (entries, current) = self.export_choices(f, field);
        if entries.is_empty() {
            return;
        }
        // Wide enough for why a format cannot be picked.
        let list = ListPicker::new(entries, None, current.as_ref()).with_max_width(96);
        self.overlays.push(Overlay::Picker(Picker::Export { field, list }));
    }

    /// What the list of choice field `field` did: a pick sets the field, while the dialog is the
    /// dialog on top once the list closed.
    pub(super) fn export_choice_picked(&mut self, field: ExportField, out: Outcome<usize>) {
        if out == Outcome::Pending {
            return;
        }
        self.overlays.close(OverlayKind::Picker);
        if let (Outcome::Picked(i), Some(OverlayKind::Export)) = (out, self.overlays.top().map(|o| o.kind())) {
            self.export_set_choice(field, i);
        }
    }

    /// Choice field `field` takes value `i` of its list.
    fn export_set_choice(&mut self, field: ExportField, i: usize) {
        let Some(f) = self.overlays.export_form_mut() else { return };
        match field {
            ExportField::Scope => {
                if let Some(s) = f.scopes.get(i) {
                    f.scope = *s;
                    f.clear_error(ExportField::Format);
                }
            }
            ExportField::Format => {
                if let Some(FormatChoice::Ready(fmt)) = format_choices().get(i) {
                    f.set_format(*fmt);
                }
            }
            ExportField::Quote => {
                if let Some(q) = QUOTES.get(i) {
                    f.quote = *q;
                }
            }
            _ => {}
        }
    }

    /// `←`/`→` on choice field `field`: the value `d` entries on that can be picked, stopping at
    /// the ends.
    fn export_step_choice(&mut self, field: ExportField, d: isize) {
        let Some(f) = self.overlays.export_form() else { return };
        let (entries, current) = self.export_choices(f, field);
        let Some(mut i) = current else { return };
        loop {
            let next = i as isize + d;
            if next < 0 || next as usize >= entries.len() {
                return;
            }
            i = next as usize;
            if !entries[i].disabled {
                return self.export_set_choice(field, i);
            }
        }
    }

    /// Keys of the export dialog (`overlay.export`, text input).
    pub(super) fn export_form_key(&mut self, key: KeyEvent, _repeat: bool) {
        let Some(f) = self.overlays.export_form_mut() else { return };
        let focus = f.focus;
        match key.code {
            KeyCode::Esc => self.close_export_dialog(),
            KeyCode::Tab | KeyCode::Down => f.step_focus(1),
            KeyCode::BackTab | KeyCode::Up => f.step_focus(-1),
            KeyCode::Enter => match focus {
                _ if focus.is_choice() => self.open_export_picker(focus),
                ExportField::Header => f.header = !f.header,
                ExportField::Cancel => self.close_export_dialog(),
                _ => self.export_form_submit(),
            },
            KeyCode::Char(' ') if !focus.is_text() => match focus {
                _ if focus.is_choice() => self.open_export_picker(focus),
                ExportField::Header => f.header = !f.header,
                ExportField::Cancel => self.close_export_dialog(),
                _ => self.export_form_submit(),
            },
            KeyCode::Left | KeyCode::Right if !focus.is_text() => {
                let d = if key.code == KeyCode::Right { 1 } else { -1 };
                match focus {
                    _ if focus.is_choice() => self.export_step_choice(focus, d),
                    ExportField::Header => f.header = !f.header,
                    ExportField::Export => f.focus = ExportField::Cancel,
                    _ => f.focus = ExportField::Export,
                }
            }
            _ => {
                if let Some(input) = f.input_mut(focus)
                    && input.handle_key(&key) == InputResult::Changed
                {
                    f.clear_error(focus);
                }
            }
        }
    }

    /// Text pasted while the export dialog has the keyboard: into the focused text field.
    pub(super) fn export_form_paste(&mut self, text: &str) {
        let Some(f) = self.overlays.export_form_mut() else { return };
        let focus = f.focus;
        if let Some(input) = f.input_mut(focus) {
            input.insert_str(text);
            f.clear_error(focus);
        }
    }

    /// Export: the dialog's request, handed to [`App::start_export`] once the dialog closed;
    /// what is wrong with it stays in the dialog, at the field to change.
    fn export_form_submit(&mut self) {
        let Some(f) = self.overlays.export_form() else { return };
        if !self.export_form_holds(f) {
            self.close_export_dialog();
            return self.flash(Notice::new(Label::ExportStale, Level::Warning));
        }
        match self.export_form_request(f) {
            Ok(req) => {
                self.close_export_dialog();
                self.start_export(req);
            }
            Err(e) => {
                let (field, n) = *e;
                if let Some(f) = self.overlays.export_form_mut() {
                    f.focus = field;
                    f.error = Some((field, n));
                }
            }
        }
    }

    fn export_form_request(&self, f: &ExportForm) -> Result<ExportRequest, Refusal> {
        let csv = f.delimited()?;
        if let Some(n) = self.export_refusal(f.scope, f.format) {
            return Err(Box::new((ExportField::Format, Notice::new(n.msg, Level::Error))));
        }
        let typed = f.path.text().trim();
        let no_path = || Box::new((ExportField::Path, Notice::new(Label::ExportDialogNoPath, Level::Error)));
        let path = expand_path(typed, self.home().as_deref(), &self.export_folder()).ok_or_else(no_path)?;
        Ok(ExportRequest {
            scope: f.scope,
            format: f.format,
            csv,
            path,
            default_path: typed == f.default_text(f.format),
            overwrite: false,
        })
    }

    /// The mouse on the export dialog: a field's line focuses it, its input puts the cursor
    /// under the pointer, a value opens its list (the header's checkbox ticks or clears), all on
    /// the press; Export and Cancel act on the release over the button pressed.
    pub(super) fn export_form_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(f) = self.overlays.export_form_mut() else { return };
        let (x, y) = (m.column, m.row);
        match f.buttons.press(m.kind, x, y, now) {
            Some(0) => {
                f.focus = ExportField::Export;
                return self.export_form_submit();
            }
            Some(_) => return self.close_export_dialog(),
            None if m.kind != MouseEventKind::Down(MouseButton::Left) || f.buttons.at(x, y).is_some() => return,
            None => {}
        }
        let hit = f.hits.iter().rev().find(|(r, _)| r.contains(Position::new(x, y))).map(|h| h.1);
        match hit {
            Some(ExportHit::Field(field)) => f.focus = field,
            Some(ExportHit::Input(field)) => {
                f.focus = field;
                if let Some(input) = f.input_mut(field) {
                    input.click(x, y);
                }
            }
            Some(ExportHit::Value(ExportField::Header)) => {
                f.focus = ExportField::Header;
                f.header = !f.header;
            }
            Some(ExportHit::Value(field)) => {
                f.focus = field;
                self.open_export_picker(field);
            }
            None => {}
        }
    }
}
