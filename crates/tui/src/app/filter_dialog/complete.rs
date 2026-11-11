//! Completion in the SQL section's fields of the filter dialog, as in `:where` and `:orderby`
//! ([`App::filter_completion`]): the `WHERE` field through the editor's engine on the statement
//! the condition goes into, the `ORDER BY` field from the table's columns. The popup opens by
//! itself after the completion delay as in the editor, or on `Ctrl+Space`; it is drawn at the
//! field's cursor, over the dialog. `Enter` and `Tab` take its entry only once the user turned
//! to it (moved its selection, or asked for it); else they apply and move on.

use super::*;
use crate::app::table_query::Field;

impl App {
    /// The SQL section's field that has the keys, while the dialog is the top overlay.
    pub(crate) fn filter_dialog_sql_field(&self) -> Option<Field> {
        if self.overlays.top().is_none_or(|o| o.kind() != OverlayKind::FilterDialog) {
            return None;
        }
        let d = self.filter_dialog()?;
        if d.sql.is_none() || d.picker.is_some() {
            return None;
        }
        match d.focus {
            DialogFocus::SqlWhere => Some(Field::Where),
            DialogFocus::SqlOrder => Some(Field::Order),
            _ => None,
        }
    }

    /// The popup of the dialog's fields closes, and no completion is due.
    pub(crate) fn filter_dialog_popup_close(&mut self) {
        let t = self.tab_mut();
        if t.popup.as_ref().is_some_and(|p| matches!(p.owner, PopupOwner::Dialog(_))) {
            t.popup = None;
            t.values_wait = None;
        }
        t.completion_due = None;
    }

    /// Completion at the cursor of the SQL section's field with the keys. `literal`: the
    /// checks' values came late, and the popup shows only if it offers values for the literal
    /// whose value starts there.
    pub(crate) fn filter_dialog_complete(&mut self, force: bool, literal: Option<usize>) {
        let Some(f) = self.filter_dialog_sql_field() else { return };
        let Some(d) = self.filter_dialog() else { return };
        let Some(input) = d.sql_field().map(|s| &s.input) else { return };
        let (tab, text, cursor) = (d.tab, input.text().to_string(), input.cursor_byte());
        let out = self.filter_completion(tab, f, &text, cursor, force);
        // One asked for stays asked for while it follows the typing.
        let touched = force || self.tab().popup.as_ref().is_some_and(|p| p.owner == PopupOwner::Dialog(f) && p.touched);
        let popup = out.completion.map(|c| Popup {
            items: c.items,
            selected: 0,
            replace_start: c.replace_start,
            trail: c.trail,
            owner: PopupOwner::Dialog(f),
            touched,
        });
        self.completed(popup, literal);
        if let Some(w) = out.wants_checks {
            self.ask_checks(w.schema, w.table, w.literal, PopupOwner::Dialog(f));
            if let Some(w) = self.tab_mut().values_wait.as_mut() {
                w.asked |= touched;
            }
        }
    }

    /// A key changed or moved through the SQL section's field (`typed`: the character it
    /// typed): an open popup follows the text, a character that starts a name, a qualified
    /// name or a literal schedules one (`[editor] completion_delay_ms`, as in the editor),
    /// anything else closes it.
    pub(super) fn filter_dialog_sql_typed(&mut self, changed: bool, typed: Option<char>) {
        let Some(tab) = self.filter_dialog().map(|d| d.tab) else { return };
        let open = self.tab().popup.as_ref().is_some_and(|p| matches!(p.owner, PopupOwner::Dialog(_)));
        let d = self.tab_dialect(tab);
        let values = self
            .tab()
            .popup
            .as_ref()
            .is_some_and(|p| p.items.first().is_some_and(|c| c.kind == datarig_core::sql::complete::Kind::Value));
        let starts = |c: char| is_ident_char(c) || c == '.' || d.string_quotes().contains(&c);
        match typed {
            _ if !changed => self.filter_dialog_popup_close(),
            Some(c) if open && (is_ident_char(c) || values) => self.filter_dialog_complete(false, None),
            Some(c) if starts(c) => {
                let pending = is_ident_char(c) && self.tab().completion_due.is_some();
                let prefix = self.filter_dialog_word_before().chars().count();
                if c == '.' || !is_ident_char(c) || pending || prefix >= COMPLETION_MIN_PREFIX {
                    let delay = Duration::from_millis(self.prefs.completion_delay.millis());
                    if delay.is_zero() {
                        self.tab_mut().completion_due = None;
                        self.filter_dialog_complete(false, None);
                    } else {
                        self.tab_mut().completion_due = Some(self.now() + delay);
                    }
                } else {
                    self.tab_mut().completion_due = None;
                }
            }
            // A deletion: an open popup follows it.
            None if open => {
                self.tab_mut().completion_due = None;
                self.filter_dialog_complete(false, None);
            }
            _ => self.filter_dialog_popup_close(),
        }
    }

    /// The name characters right before the cursor of the SQL section's field with the keys.
    fn filter_dialog_word_before(&self) -> String {
        let Some(input) = self.filter_dialog().and_then(|d| d.sql_field()).map(|s| &s.input) else {
            return String::new();
        };
        let before = &input.text()[..input.cursor_byte()];
        let start =
            before.char_indices().rev().take_while(|(_, c)| is_ident_char(*c)).last().map_or(before.len(), |(i, _)| i);
        before[start..].to_string()
    }
}
