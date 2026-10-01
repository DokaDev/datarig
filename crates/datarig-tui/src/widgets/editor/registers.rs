//! Registers, as Vim keeps them: the unnamed one (`"`, the register written last), `"a`–`"z`
//! (`"A`–`"Z` append), `"0` (the last yank), `"1`–`"9` (deletes of whole lines or of more than
//! one line, shifted down by each new one), `"-` (smaller deletes), `"_` (throws the text away)
//! and `"+` / `"*` (the system clipboard: the editor keeps what went there or what the app read
//! from it for a put, the app does the reading and writing). `".` holds the text the last
//! Insert session typed; `":` `"%` `"#` `"/` are names Vim knows that hold nothing here.
//!
//! A write that Vim with `clipboard=unnamedplus` would send to the system clipboard (any yank,
//! delete or change without a register, and `"+` / `"*`) is offered to the app as a [`Yank`].

/// How a register's text is put: by character, as whole lines, or as a block (a rectangle,
/// one piece per line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegKind {
    Charwise,
    Linewise,
    Blockwise,
}

/// What a register holds. By line and as a block, `text` has the lines without a line break
/// after the last one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub kind: RegKind,
}

impl Register {
    pub fn new(text: impl Into<String>, kind: RegKind) -> Self {
        Register { text: text.into(), kind }
    }

    /// Whole lines (`yy`, `dj`, `V`…): put as lines of their own.
    pub fn linewise(&self) -> bool {
        self.kind == RegKind::Linewise
    }

    /// The text as the system clipboard gets it: lines and blocks end with a line break, as
    /// Vim writes them there.
    pub fn clipboard_text(&self) -> String {
        match self.kind {
            RegKind::Charwise => self.text.clone(),
            RegKind::Linewise | RegKind::Blockwise => format!("{}\n", self.text),
        }
    }

    /// A register from the system clipboard's text: whole lines when it ends with a line break
    /// (Vim reads it so), by character otherwise. `\r\n` and `\r` become `\n`.
    pub fn from_clipboard(text: &str) -> Self {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        match text.strip_suffix('\n') {
            Some(lines) => Register::new(lines, RegKind::Linewise),
            None => Register::new(text, RegKind::Charwise),
        }
    }

    /// `self` with `more` appended (`"A`): whole lines when `more` is, else the kind stays;
    /// text by character is joined to the last line, anything else goes on lines of its own.
    fn append(self, more: Register) -> Register {
        let kind = if more.kind == RegKind::Linewise { RegKind::Linewise } else { self.kind };
        let text = match kind {
            RegKind::Charwise => self.text + &more.text,
            _ => format!("{}\n{}", self.text, more.text),
        };
        Register { text, kind }
    }
}

/// Why a command did nothing with a register: register `.0` is empty (a put, `Ctrl+R`), or
/// takes no yank or delete (`".`, `"%`… are filled by the editor or not kept here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegProblem {
    Empty(char),
    ReadOnly(char),
}

/// A register write for the system clipboard: `register` is `"` when the command named no
/// register (it goes there when the `[editor] clipboard` setting is on), or `+` / `*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yank {
    pub register: char,
    pub reg: Register,
}

/// Whether `c` names a register a command may take (`"x`): the ones kept here, and the ones
/// Vim fills itself (`.` `:` `%` `#` `/`), which are only read and hold nothing here.
pub(super) fn is_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '"' | '-' | '_' | '+' | '*' | '.' | ':' | '%' | '#' | '/')
}

/// A register a yank or a delete may write.
fn writable(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '"' | '-' | '_' | '+' | '*')
}

/// A register named by the command: its write goes to the system clipboard.
fn is_clipboard(c: char) -> bool {
    matches!(c, '+' | '*')
}

/// The registers of an editor.
#[derive(Debug, Default)]
pub(super) struct Registers {
    named: [Option<Register>; 26],
    /// `"0`, then the delete ring `"1`–`"9`.
    numbered: [Option<Register>; 10],
    small: Option<Register>,
    clipboard: Option<Register>,
    /// `".`: what the last Insert session typed.
    inserted: Option<Register>,
    /// The register the unnamed one stands for (the one written last).
    last: Option<char>,
}

impl Registers {
    /// The register named `c` (`"` the unnamed one; `A`–`Z` read as `a`–`z`).
    pub(super) fn get(&self, c: char) -> Option<&Register> {
        match c {
            '"' => self.last.and_then(|l| self.get(l)),
            'a'..='z' => self.named[c as usize - 'a' as usize].as_ref(),
            'A'..='Z' => self.named[c as usize - 'A' as usize].as_ref(),
            '0'..='9' => self.numbered[c as usize - '0' as usize].as_ref(),
            '-' => self.small.as_ref(),
            '+' | '*' => self.clipboard.as_ref(),
            '.' => self.inserted.as_ref(),
            _ => None,
        }
    }

    /// The system clipboard's text as the app read it for a put.
    pub(super) fn set_clipboard(&mut self, reg: Register) {
        self.clipboard = Some(reg);
    }

    /// What the last Insert session typed (`".`).
    pub(super) fn set_inserted(&mut self, text: String) {
        self.inserted = Some(Register::new(text, RegKind::Charwise));
    }

    /// Put `reg` in register `c`, appending for `A`–`Z`; it becomes the unnamed register.
    fn write(&mut self, c: char, reg: Register) {
        let slot = match c {
            'a'..='z' => &mut self.named[c as usize - 'a' as usize],
            'A'..='Z' => {
                let slot = &mut self.named[c.to_ascii_lowercase() as usize - 'a' as usize];
                *slot = Some(match slot.take() {
                    Some(old) => old.append(reg),
                    None => reg,
                });
                self.last = Some(c.to_ascii_lowercase());
                return;
            }
            '0'..='9' => &mut self.numbered[c as usize - '0' as usize],
            '-' => &mut self.small,
            '+' | '*' => &mut self.clipboard,
            _ => return,
        };
        *slot = Some(reg);
        self.last = Some(if c == '*' { '+' } else { c });
    }

    /// Whether a yank or a delete into `name` does anything (a register Vim fills itself does
    /// not take one: the command fails).
    pub(super) fn can_write(name: Option<char>) -> bool {
        name.is_none_or(writable)
    }

    /// A yank into `name` (none named: `"0`); what goes to the system clipboard.
    pub(super) fn yank(&mut self, name: Option<char>, reg: Register) -> Option<Yank> {
        match name.filter(|&c| c != '"') {
            Some('_') => None,
            Some(c) => {
                self.write(c, reg.clone());
                is_clipboard(c).then_some(Yank { register: c, reg })
            }
            None => {
                self.write('0', reg.clone());
                Some(Yank { register: '"', reg })
            }
        }
    }

    /// A delete (or change) into `name`; `ring`: it goes to `"1` as well, the older deletes
    /// moving down (whole lines, more than one line, or a motion Vim does it for). Without a
    /// register, a delete within a line goes to `"-`. What goes to the system clipboard.
    pub(super) fn delete(&mut self, name: Option<char>, reg: Register, ring: bool) -> Option<Yank> {
        let name = name.filter(|&c| c != '"');
        if name == Some('_') {
            return None;
        }
        if let Some(c) = name {
            self.write(c, reg.clone());
        }
        if ring {
            self.numbered[1..].rotate_right(1);
            self.numbered[1] = Some(reg.clone());
            // An append keeps the unnamed register on the whole appended text.
            if !name.is_some_and(|c| c.is_ascii_uppercase()) {
                self.last = Some('1');
            }
        }
        if name.is_none() && reg.kind != RegKind::Linewise && !reg.text.contains('\n') {
            self.write('-', reg.clone());
        }
        match name {
            None => Some(Yank { register: '"', reg }),
            Some(c) if is_clipboard(c) => Some(Yank { register: c, reg }),
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests;
