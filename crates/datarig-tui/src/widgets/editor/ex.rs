//! The editor's Ex commands, typed on the app's `:` command line: a line range (`12`, `.`,
//! `$`, `'a`, `'<`, `%`, with `+n` / `-n` offsets, `a,b` and `a;b`), alone to go to its last
//! line, or before `:s` (`:substitute`), `:&` and `:&&`; and `&` (as Neovim's, with the
//! flags) / `g&` in Normal mode.
//!
//! `:s/pattern/replacement/flags`: the pattern is a Rust regular expression, as search's is (an
//! empty one is the last search's); a match lies within one line. The replacement is Vim's:
//! `&` and `\0` the match, `\1`–`\9` its groups (an absent one is empty), `~` the previous
//! replacement, `\r` and `\n` a line break, `\t` a tab, `\u` `\l` the next character's case,
//! `\U` `\L` up to `\E` / `\e`, a backslash before any other character that character (`\&`,
//! `\~`, `\\`, the delimiter). `$` is plain text. The flags are `g` (every match of a line, not
//! the first), `i` / `I` (ignore case or not), `e` (no error when nothing matches) and `&`
//! (first: the last flags again); `c` (confirm) is not supported. Any delimiter that is not a
//! letter, a digit, a blank, `\`, `"` or `|` works. One `:s` is one undo step.

use super::marks::MarkNotice;
use super::motion::Pos;
use super::search::{SearchNotice, summary};
use super::{EdEvent, Editor};
use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

/// Why an Ex command did not run (or found nothing), for the app to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExError {
    /// Not a command the editor takes (Vim has more).
    Unsupported(String),
    /// A line of the range is past the text.
    InvalidRange,
    /// The range's first line is after its last.
    Backwards,
    Mark(MarkNotice),
    /// `:s` with the `c` flag.
    Confirm,
    /// Text after the flags that is not one of them.
    Trailing(String),
    /// What a search would say: an invalid pattern, no match, no pattern before.
    Pattern(SearchNotice),
}

/// What an Ex command did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExDone {
    /// The cursor went to a line.
    Moved,
    /// `count` matches replaced on `lines` lines.
    Substituted { count: usize, lines: usize },
    /// Nothing matched, and the `e` flag said not to mind.
    Nothing,
}

/// The flags of a substitute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Flags {
    all: bool,
    /// `i` (true) or `I` (false); none: the pattern decides.
    ignore_case: Option<bool>,
    quiet: bool,
}

/// The last substitute, for `:&`, `&` and `g&`: its pattern as Rust regex text, its
/// replacement (with `~` already put in) and its flags.
#[derive(Clone, Debug)]
pub(super) struct LastSub {
    pattern: String,
    replacement: String,
    flags: Flags,
}

/// Whether `text` (what follows `:`) is an editor command: it starts with a line address
/// (`12`, `.`, `$`, `%`, `'a`, `+`, `-`, `,`, `;`), with `&`, or is `s` (or a longer start of
/// `substitute`) alone or followed by something that is not a letter.
pub fn is_ex(text: &str) -> bool {
    let t = text.trim_start();
    let Some(c) = t.chars().next() else { return false };
    if c.is_ascii_digit() || matches!(c, '.' | '$' | '%' | '\'' | '+' | '-' | ',' | ';' | '&') {
        return true;
    }
    let name: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
    !name.is_empty() && "substitute".starts_with(&name)
}

/// A line address in Vim's numbers (1 is the first line, 0 before it).
type Line = isize;

/// A range's first and last lines, if one was typed.
type Range = Option<(Line, Line)>;

/// What the command line holds after the range.
enum Command<'a> {
    /// Nothing: go to the range's last line.
    Go,
    /// `:s` and what follows it (`None`: `:s` alone or with flags only).
    Substitute(&'a str),
    /// `:&` (`keep`: `:&&`, with the last flags) and the flags after it.
    Again { keep: bool, flags: &'a str },
}

impl Editor {
    /// Run Ex command `text` (what follows `:`). The cursor's place before a jump or a
    /// substitute becomes the context mark, as in Vim.
    pub fn ex(&mut self, text: &str) -> Result<ExDone, ExError> {
        let (range, rest) = self.ex_range(text.trim())?;
        let command = command(rest.trim_start())?;
        let cursor = (self.row, self.col);
        let done = match command {
            Command::Go => {
                let Some((_, last)) = range else { return Ok(ExDone::Moved) };
                if last < 0 {
                    return Err(ExError::InvalidRange);
                }
                // Line 0 is the first; past the end, the last (Vim).
                let r = (last.max(1) as usize - 1).min(self.lines.len() - 1);
                self.marks.set_pc(cursor);
                self.set_pos(r, self.first_nonblank(r));
                ExDone::Moved
            }
            Command::Substitute(args) => {
                let lines = self.lines_of(range)?;
                self.substitute_args(args, lines)?
            }
            Command::Again { keep, flags } => {
                let lines = self.lines_of(range)?;
                let mut f = parse_flags(flags)?;
                let last = self.last_sub.clone().ok_or(ExError::Pattern(SearchNotice::NoPrevious))?;
                if keep {
                    f = Flags {
                        all: f.all != last.flags.all,
                        ignore_case: f.ignore_case.or(last.flags.ignore_case),
                        quiet: f.quiet || last.flags.quiet,
                    };
                }
                self.substitute(&last.pattern, &last.replacement, f, lines)?
            }
        };
        self.marks.check_pc((self.row, self.col));
        Ok(done)
    }

    /// `&` (`:s` again on the cursor's line with its flags, as Neovim's `:&&`) and `g&`
    /// (`all`: every line, the last search's pattern, the last flags). Not repeated by `.`.
    pub(super) fn sub_again(&mut self, all: bool) -> EdEvent {
        self.rec.skip();
        let version = self.version;
        let r = match (all, self.last_sub.clone()) {
            (_, None) => Err(ExError::Pattern(SearchNotice::NoPrevious)),
            (false, Some(last)) => self.substitute(&last.pattern, &last.replacement, last.flags, (self.row, self.row)),
            (true, Some(last)) => {
                let pattern = self.last_search.as_ref().map_or(last.pattern.clone(), |l| l.re.as_str().to_string());
                self.substitute(&pattern, &last.replacement, last.flags, (0, self.lines.len() - 1))
            }
        };
        if let Err(ExError::Pattern(n)) = r {
            self.search_notice = Some(n);
        }
        if self.version != version { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }

    /// The range at the start of `text` (as Vim's line numbers, first and last) and the rest.
    fn ex_range<'a>(&self, text: &'a str) -> Result<(Range, &'a str), ExError> {
        if let Some(rest) = text.strip_prefix('%') {
            return Ok((Some((1, self.lines.len() as Line)), rest));
        }
        let cursor = self.row as Line + 1;
        let (Some(a), rest) = self.address(text, cursor)? else { return Ok((None, text)) };
        let mut s = rest.trim_start();
        let Some(sep) = s.chars().next().filter(|c| matches!(c, ',' | ';')) else { return Ok((Some((a, a)), s)) };
        s = &s[1..];
        // After `;` the second address counts from the first.
        let base = if sep == ';' { a } else { cursor };
        match self.address(s, base)? {
            (Some(b), rest) => Ok((Some((a, b)), rest)),
            (None, rest) => Ok((Some((a, a)), rest)),
        }
    }

    /// One address at the start of `s` (`base` is `.`) and the rest; `None` when there is
    /// none.
    fn address<'a>(&self, s: &'a str, base: Line) -> Result<(Option<Line>, &'a str), ExError> {
        let s = s.trim_start();
        let digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let (mut at, mut rest) = match s.chars().next() {
            Some(c) if c.is_ascii_digit() => (Some(number(&s[..digits])), &s[digits..]),
            Some('.') => (Some(base), &s[1..]),
            Some('$') => (Some(self.lines.len() as Line), &s[1..]),
            Some('\'') => {
                let Some(m) = s[1..].chars().next() else { return Err(ExError::Unsupported(s.to_string())) };
                let (r, _) = self.marks.get(m).map_err(ExError::Mark)?;
                (Some(r as Line + 1), &s[1 + m.len_utf8()..])
            }
            _ => (None, s),
        };
        // Offsets: `+3`, `-`, `++` (one each without a number).
        loop {
            let t = rest.trim_start();
            let Some(sign) = t.chars().next().filter(|c| matches!(c, '+' | '-')) else { break };
            let t = &t[1..];
            let n = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            let by = if n == 0 { 1 } else { number(&t[..n]) };
            let from = at.unwrap_or(base);
            at = Some(if sign == '+' { from.saturating_add(by) } else { from.saturating_sub(by) });
            rest = &t[n..];
        }
        Ok((at, rest))
    }

    /// The lines (from 0) a substitute takes: the range's, or the cursor's.
    fn lines_of(&self, range: Range) -> Result<(usize, usize), ExError> {
        let Some((a, b)) = range else { return Ok((self.row, self.row)) };
        let n = self.lines.len() as Line;
        if a < 0 || b < 0 || a > n || b > n {
            return Err(ExError::InvalidRange);
        }
        if a > b {
            return Err(ExError::Backwards);
        }
        Ok(((a.max(1) - 1) as usize, (b.max(1) - 1) as usize))
    }

    /// `:s` with what follows it, over `lines`.
    fn substitute_args(&mut self, args: &str, lines: (usize, usize)) -> Result<ExDone, ExError> {
        let args = args.trim_start();
        let delim = args.chars().next().filter(|&c| delimiter(c));
        let Some(delim) = delim else {
            // `:s` alone or with flags: the last one again with those flags.
            let flags = parse_flags(args)?;
            let last = self.last_sub.clone().ok_or(ExError::Pattern(SearchNotice::NoPrevious))?;
            return self.substitute(&last.pattern, &last.replacement, flags, lines);
        };
        let body = &args[delim.len_utf8()..];
        let (pattern, rest) = split(body, delim, true);
        let (replacement, flags) = match rest {
            Some(rest) => {
                let (r, f) = split(rest, delim, false);
                (r, f.unwrap_or(""))
            }
            None => (String::new(), ""),
        };
        let flags = parse_flags(flags)?;
        // `~` is the previous replacement.
        let replacement = tilde(&replacement, self.last_sub.as_ref().map_or("", |l| l.replacement.as_str()));
        let pattern = match pattern.as_str() {
            "" => self
                .last_search
                .as_ref()
                .map(|l| l.re.as_str().to_string())
                .ok_or(ExError::Pattern(SearchNotice::NoPrevious))?,
            p => p.to_string(),
        };
        self.substitute(&pattern, &replacement, flags, lines)
    }

    /// Replace the matches of `pattern` in `lines` with `replacement` as one undo step, the
    /// cursor then on the last line changed (its first non-blank). The pattern becomes the
    /// last search's, the substitute the last one.
    fn substitute(
        &mut self,
        pattern: &str,
        replacement: &str,
        flags: Flags,
        (first, last): (usize, usize),
    ) -> Result<ExDone, ExError> {
        let source = match flags.ignore_case {
            Some(true) => format!("(?i){pattern}"),
            Some(false) => format!("(?-i){pattern}"),
            None => pattern.to_string(),
        };
        let re = Regex::new(&source).map_err(|e| ExError::Pattern(SearchNotice::Invalid(summary(&e))))?;
        let plain = Regex::new(pattern).map_err(|e| ExError::Pattern(SearchNotice::Invalid(summary(&e))))?;
        let forward = self.last_search.as_ref().is_none_or(|l| l.forward());
        self.set_search(plain, forward);
        self.last_sub = Some(LastSub { pattern: pattern.to_string(), replacement: replacement.to_string(), flags });
        let template = parse_replacement(replacement);
        // The lines that change, from the first to the last: (line, new text).
        let mut changed: Vec<(usize, String)> = Vec::new();
        let mut count = 0;
        for r in first..=last {
            let line = &self.lines[r];
            self.search_work.bytes += line.len() + 1;
            if let Some((new, n)) = replace_line(&re, line, &template, flags.all) {
                count += n;
                changed.push((r, new));
            }
        }
        let (Some(&(a, _)), Some(&(b, _))) = (changed.first(), changed.last()) else {
            if flags.quiet {
                return Ok(ExDone::Nothing);
            }
            return Err(ExError::Pattern(SearchNotice::NotFound(pattern.to_string())));
        };
        let mut text = String::new();
        let mut it = changed.iter().peekable();
        for r in a..=b {
            if r > a {
                text.push('\n');
            }
            match it.peek() {
                Some((cr, new)) if *cr == r => {
                    text.push_str(new);
                    it.next();
                }
                _ => text.push_str(&self.lines[r]),
            }
        }
        let cursor: Pos = (self.row, self.col);
        self.marks.set_pc(cursor);
        // Vim's undo goes back to the first changed line's start.
        (self.row, self.col) = (a, 0);
        self.snapshot();
        let (start, _) = self.line_bounds(a);
        let (_, end) = self.line_bounds(b);
        self.splice(start, end, &text);
        // The last line changed is the text's last, after the line breaks added before it.
        let last_row = a + text.matches('\n').count();
        self.set_pos(last_row, self.first_nonblank(last_row));
        Ok(ExDone::Substituted { count, lines: changed.len() })
    }
}

/// What follows the range.
fn command(s: &str) -> Result<Command<'_>, ExError> {
    if s.is_empty() {
        return Ok(Command::Go);
    }
    if let Some(rest) = s.strip_prefix('&') {
        return Ok(match rest.strip_prefix('&') {
            Some(flags) => Command::Again { keep: true, flags },
            None => Command::Again { keep: false, flags: rest },
        });
    }
    let name_len = s.len() - s.trim_start_matches(|c: char| c.is_ascii_alphabetic()).len();
    let name = &s[..name_len];
    if !name.is_empty() && "substitute".starts_with(name) {
        return Ok(Command::Substitute(&s[name_len..]));
    }
    Err(ExError::Unsupported(s.to_string()))
}

/// A number of a line address (a huge one saturates).
fn number(digits: &str) -> Line {
    digits.parse().unwrap_or(Line::MAX)
}

/// A character that may delimit `:s`'s pattern.
fn delimiter(c: char) -> bool {
    !c.is_alphanumeric() && !c.is_whitespace() && !matches!(c, '\\' | '"' | '|')
}

/// The text up to the first `delim` not after a backslash, and what follows it (`None`: no
/// delimiter, the text ran to the end). An escaped delimiter becomes the delimiter itself (in
/// a `pattern`, escaped for the regex when it is one of its special characters; in a
/// replacement, escaped when it is one of the replacement's).
fn split(s: &str, delim: char, pattern: bool) -> (String, Option<&str>) {
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == delim {
            return (out, Some(&s[i + c.len_utf8()..]));
        }
        if c == '\\' {
            match chars.next() {
                Some((_, d)) if d == delim => {
                    let special = if pattern { regex_syntax_meta(d) } else { matches!(d, '&' | '~') };
                    if special {
                        out.push('\\');
                    }
                    out.push(d);
                }
                Some((_, d)) => {
                    out.push('\\');
                    out.push(d);
                }
                None => out.push('\\'),
            }
            continue;
        }
        out.push(c);
    }
    (out, None)
}

/// A character the regex syntax gives a meaning (escaped to be itself).
fn regex_syntax_meta(c: char) -> bool {
    regex::escape(c.encode_utf8(&mut [0; 4])).len() > c.len_utf8()
}

fn parse_flags(s: &str) -> Result<Flags, ExError> {
    let mut f = Flags::default();
    for (i, c) in s.trim().char_indices() {
        match c {
            '&' if i == 0 => {}
            'g' => f.all = !f.all,
            'i' => f.ignore_case = Some(true),
            'I' => f.ignore_case = Some(false),
            'e' => f.quiet = true,
            'c' => return Err(ExError::Confirm),
            _ => return Err(ExError::Trailing(s.trim()[i..].to_string())),
        }
    }
    Ok(f)
}

/// `replacement` with each `~` (not after a backslash) replaced by `previous`.
fn tilde(replacement: &str, previous: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                if let Some(d) = chars.next() {
                    out.push(d);
                }
            }
            '~' => out.push_str(previous),
            c => out.push(c),
        }
    }
    out
}

/// A piece of a replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Text(String),
    /// A group of the match (0: the whole match).
    Group(usize),
    /// `\u` (true) or `\l`: the next character's case.
    One(bool),
    /// `\U` (Some(true)), `\L`, or `\E` (None): the case from here on.
    All(Option<bool>),
}

fn parse_replacement(s: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut chars = s.chars();
    let flush = |text: &mut String, out: &mut Vec<Piece>| {
        if !text.is_empty() {
            out.push(Piece::Text(std::mem::take(text)));
        }
    };
    while let Some(c) = chars.next() {
        let piece = match c {
            '&' => Piece::Group(0),
            '\\' => match chars.next() {
                Some(d @ '0'..='9') => Piece::Group(d as usize - '0' as usize),
                Some('r' | 'n') => {
                    text.push('\n');
                    continue;
                }
                Some('t') => {
                    text.push('\t');
                    continue;
                }
                Some('u') => Piece::One(true),
                Some('l') => Piece::One(false),
                Some('U') => Piece::All(Some(true)),
                Some('L') => Piece::All(Some(false)),
                Some('E' | 'e') => Piece::All(None),
                Some(d) => {
                    text.push(d);
                    continue;
                }
                None => {
                    text.push('\\');
                    continue;
                }
            },
            c => {
                text.push(c);
                continue;
            }
        };
        flush(&mut text, &mut out);
        out.push(piece);
    }
    flush(&mut text, &mut out);
    out
}

/// The text `template` makes of a match.
fn expand(template: &[Piece], caps: &regex::Captures) -> String {
    let mut out = String::new();
    let mut one: Option<bool> = None;
    let mut all: Option<bool> = None;
    let push = |s: &str, out: &mut String, one: &mut Option<bool>, all: Option<bool>| {
        for c in s.chars() {
            let case = one.take().or(all);
            match case {
                Some(true) => out.extend(c.to_uppercase()),
                Some(false) => out.extend(c.to_lowercase()),
                None => out.push(c),
            }
        }
    };
    for p in template {
        match p {
            Piece::Text(t) => push(t, &mut out, &mut one, all),
            Piece::Group(g) => push(caps.get(*g).map_or("", |m| m.as_str()), &mut out, &mut one, all),
            Piece::One(up) => one = Some(*up),
            Piece::All(case) => all = *case,
        }
    }
    out
}

/// `line` with its first match (every one with `all`) replaced, and how many; `None` when it
/// has none. After an empty match the search goes on one character later, and stops at the end
/// of the line, as Vim's does.
fn replace_line(re: &Regex, line: &str, template: &[Piece], all: bool) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut copied = 0;
    let mut at = 0;
    let mut last_end: Option<usize> = None;
    let mut n = 0;
    while at <= line.len() {
        let Some(caps) = re.captures_at(line, at) else { break };
        let m = caps.get(0).expect("group 0 is the match");
        if m.is_empty() && last_end == Some(m.start()) {
            // An empty match right after a match is not one (Vim, and Rust's own iteration).
            match next_char(line, m.start()) {
                Some(next) => {
                    at = next;
                    continue;
                }
                None => break,
            }
        }
        out.push_str(&line[copied..m.start()]);
        out.push_str(&expand(template, &caps));
        copied = m.end();
        last_end = Some(m.end());
        n += 1;
        if !all {
            break;
        }
        at = if m.is_empty() {
            match next_char(line, m.end()) {
                Some(next) if next < line.len() => next,
                _ => break,
            }
        } else {
            m.end()
        };
    }
    if n == 0 {
        return None;
    }
    out.push_str(&line[copied..]);
    Some((out, n))
}

/// The byte after the grapheme at byte `at` of `line`; `None` at its end.
fn next_char(line: &str, at: usize) -> Option<usize> {
    line.get(at..).and_then(|t| t.graphemes(true).next()).map(|g| at + g.len())
}

#[cfg(test)]
mod tests;
