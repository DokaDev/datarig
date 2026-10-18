//! The formats of [`super`] written as a stream: the head when the writer begins, the rows a
//! chunk at a time as they come, the end when it ends, into any [`io::Write`] (a file, or
//! memory for a copy, [`super::Writer`]). Nothing is held beyond the chunk being written, so an
//! export of millions of rows takes as much memory as one chunk.
//!
//! The text is byte for byte the text of the functions of [`super`] for all the rows at once,
//! whatever the chunks: the head that a format writes with its first rows (the header line of
//! TSV and CSV, the `[` of JSON) is written by [`TextWriter::begin`] or with the first rows, the
//! end (`]`, `</table>`) by [`TextWriter::end`]. Indented JSON is indented as it streams
//! ([`Indenter`]), with the same result as indenting the whole text.
//!
//! A file export adds what a file wants and a copy does not: a line break at the end of the
//! text ([`TextWriter::trailing_newline`]), CSV's options ([`Delimited`]) and JSON Lines.

use super::{
    Column, Format, HTML_END, Kind, Row, XML_END, XML_HEAD, comma_list, csv, html_head, html_rows, json_object,
    json_objects, markdown_header, markdown_rows, sql_insert, sql_number, sql_update, tsv, unique_names, xml_rows,
};
use std::io::{self, Write};

/// Where rows go, a chunk at a time: [`TextWriter`] for the text formats (and, later, the
/// binary ones behind the same calls). `begin` once, `rows` for every chunk in order, `end`
/// once; the counts say how far it got.
pub trait RowSink {
    /// Write what comes before the rows (a head, a header line).
    fn begin(&mut self) -> io::Result<()>;
    /// Write the next rows.
    fn rows(&mut self, rows: &[Row]) -> io::Result<()>;
    /// Write what comes after the rows and flush: the output is complete (also after fewer
    /// rows than there were, for an export that was stopped).
    fn end(&mut self) -> io::Result<()>;
    /// Rows written so far.
    fn rows_written(&self) -> u64;
    /// Bytes written so far.
    fn bytes(&self) -> u64;
}

/// How [`Delimited`] quotes a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quote {
    /// Only where needed: a value with the delimiter, a double quote or a line break, the empty
    /// string (NULL is then told apart) and a value that reads as the NULL text (RFC 4180).
    Necessary,
    /// Every value and name.
    Always,
    /// Every value but the numbers of a number column, and every name.
    NonNumeric,
    /// Never: values are written as they are. A value with the delimiter, a double quote or a
    /// line break then breaks the file's columns; [`TextWriter::damaged`] counts them, so the
    /// export says how many.
    Never,
}

/// The options of delimited text (CSV) for a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delimited {
    /// Between the values of a line (`,`, `;`, a tab, …).
    pub delimiter: char,
    pub quote: Quote,
    /// A first line with the column names.
    pub header: bool,
    /// What NULL is written as (never quoted): empty by default.
    pub null: String,
}

impl Default for Delimited {
    /// CSV as RFC 4180 has it, with a header line and NULL as an empty field: the text of
    /// [`super::csv`].
    fn default() -> Self {
        Self { delimiter: ',', quote: Quote::Necessary, header: true, null: String::new() }
    }
}

/// Why [`Delimited`] options cannot be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadDelimited {
    /// The delimiter is a double quote, a line break or a carriage return: no reader could
    /// tell the values apart.
    Delimiter,
    /// The NULL text holds the delimiter, a double quote or a line break.
    Null,
}

impl Delimited {
    /// Whether the options make a file a reader can split again.
    pub fn check(&self) -> Result<(), BadDelimited> {
        if matches!(self.delimiter, '"' | '\n' | '\r') {
            return Err(BadDelimited::Delimiter);
        }
        if self.null.contains([self.delimiter, '"', '\n', '\r']) {
            return Err(BadDelimited::Null);
        }
        Ok(())
    }

    /// Whether `v` needs quotes to be read back as it is.
    fn needs_quotes(&self, v: &str) -> bool {
        v.is_empty() || v.contains([self.delimiter, '"', '\n', '\r']) || v == self.null
    }

    /// Whether `v` breaks the columns when it is written without quotes.
    fn breaks(&self, v: &str) -> bool {
        v.contains([self.delimiter, '"', '\n', '\r'])
    }

    /// One field: `v` of a column of kind `kind` (`None` for a name in the header).
    fn field(&self, v: &str, kind: Option<Kind>, damaged: &mut u64) -> String {
        let quote = match self.quote {
            Quote::Necessary => self.needs_quotes(v),
            Quote::Always => true,
            Quote::NonNumeric => !(kind == Some(Kind::Number) && sql_number(v)),
            Quote::Never => {
                if self.breaks(v) {
                    *damaged += 1;
                }
                false
            }
        };
        if quote { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
    }

    /// The header line.
    fn header_line(&self, columns: &[Column], damaged: &mut u64) -> String {
        let sep = self.delimiter.to_string();
        columns.iter().map(|c| self.field(c.name, None, damaged)).collect::<Vec<_>>().join(&sep)
    }

    /// One line per row, joined by `\n`.
    fn lines(&self, columns: &[Column], rows: &[Row], damaged: &mut u64) -> String {
        let sep = self.delimiter.to_string();
        rows.iter()
            .map(|r| {
                r.iter()
                    .zip(columns)
                    .map(|(v, c)| match v {
                        Some(v) => self.field(v, Some(c.kind), damaged),
                        None => self.null.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(&sep)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Indents JSON text fed a piece at a time, as `super::indent_json` indents the whole text:
/// every token kept, each object and array on lines of its own, two spaces a level, an empty
/// object or array on its line. The one look-ahead (is the bracket just opened empty?) waits
/// for the next character that is not white space.
#[derive(Debug, Default)]
pub struct Indenter {
    depth: usize,
    in_str: bool,
    escaped: bool,
    /// An opening bracket was written and the next character that is not white space decides
    /// whether it is empty.
    open: bool,
}

impl Indenter {
    fn newline(&self, out: &mut String) {
        out.push('\n');
        for _ in 0..self.depth {
            out.push_str("  ");
        }
    }

    /// The indented text of the next piece `s`.
    pub fn feed(&mut self, s: &str, out: &mut String) {
        for c in s.chars() {
            if self.in_str {
                out.push(c);
                if self.escaped {
                    self.escaped = false;
                } else if c == '\\' {
                    self.escaped = true;
                } else if c == '"' {
                    self.in_str = false;
                }
                continue;
            }
            if self.open {
                if c.is_whitespace() {
                    continue;
                }
                self.open = false;
                if matches!(c, '}' | ']') {
                    out.push(c);
                    continue;
                }
                self.depth += 1;
                self.newline(out);
            }
            match c {
                '"' => {
                    self.in_str = true;
                    out.push(c);
                }
                '{' | '[' => {
                    out.push(c);
                    self.open = true;
                }
                '}' | ']' => {
                    self.depth = self.depth.saturating_sub(1);
                    self.newline(out);
                    out.push(c);
                }
                ',' => {
                    out.push(c);
                    self.newline(out);
                }
                ':' => out.push_str(": "),
                c if c.is_whitespace() => {}
                c => out.push(c),
            }
        }
    }

    /// The text is over: a bracket left open without anything after it gets its line.
    pub fn finish(&mut self, out: &mut String) {
        if std::mem::take(&mut self.open) {
            self.depth += 1;
            self.newline(out);
        }
    }
}

/// The formats of [`Format`] as a stream into `out` ([`RowSink`]).
pub struct TextWriter<'a, W: Write> {
    format: Format<'a>,
    columns: &'a [Column<'a>],
    out: W,
    rows: u64,
    bytes: u64,
    /// The last byte written.
    last: Option<u8>,
    /// Something was written that the next line of a line format goes after (the header
    /// line, earlier rows).
    lines: bool,
    trailing_newline: bool,
    indent: Option<Indenter>,
    /// The keys of a JSON Lines object.
    names: Vec<String>,
    damaged: u64,
}

impl<'a, W: Write> TextWriter<'a, W> {
    pub fn new(format: Format<'a>, columns: &'a [Column<'a>], out: W) -> Self {
        let indent = (format == Format::JsonPretty).then(Indenter::default);
        let names = if format == Format::JsonLines { unique_names(columns) } else { Vec::new() };
        Self {
            format,
            columns,
            out,
            rows: 0,
            bytes: 0,
            last: None,
            lines: false,
            trailing_newline: false,
            indent,
            names,
            damaged: 0,
        }
    }

    /// End the text with a line break (a file's last line), unless it is empty or ends with
    /// one already. A copy has none.
    pub fn trailing_newline(mut self, on: bool) -> Self {
        self.trailing_newline = on;
        self
    }

    /// Values written without the quotes they needed ([`Quote::Never`]): each breaks the
    /// columns of its line for a reader.
    pub fn damaged(&self) -> u64 {
        self.damaged
    }

    /// The output, once the writer is done with it.
    pub fn into_inner(self) -> W {
        self.out
    }

    fn put(&mut self, s: &str) -> io::Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        match self.indent.as_mut() {
            Some(ind) => {
                let mut text = String::with_capacity(s.len() * 2);
                ind.feed(s, &mut text);
                self.raw(&text)
            }
            None => self.raw(s),
        }
    }

    fn raw(&mut self, s: &str) -> io::Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        self.out.write_all(s.as_bytes())?;
        self.bytes += s.len() as u64;
        self.last = s.as_bytes().last().copied();
        Ok(())
    }

    /// The next line of a line format: after a line break when something came before it.
    fn line(&mut self, s: &str) -> io::Result<()> {
        if self.lines {
            self.raw("\n")?;
        }
        self.lines = true;
        self.raw(s)
    }
}

impl<W: Write> RowSink for TextWriter<'_, W> {
    fn begin(&mut self) -> io::Result<()> {
        let columns = self.columns;
        match &self.format {
            Format::Tsv { header: true } => {
                let head = tsv(columns, &[], true);
                self.line(&head)
            }
            Format::Csv { header: true } => {
                let head = csv(columns, &[], true);
                self.line(&head)
            }
            Format::Delimited(d) if d.header => {
                let mut damaged = 0;
                let head = d.header_line(columns, &mut damaged);
                self.damaged += damaged;
                self.line(&head)
            }
            Format::Markdown => {
                let head = markdown_header(columns);
                self.line(&head)
            }
            Format::Html => {
                let head = html_head(columns);
                self.raw(&head)
            }
            Format::Xml => self.raw(XML_HEAD),
            _ => Ok(()),
        }
    }

    fn rows(&mut self, rows: &[Row]) -> io::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let columns = self.columns;
        let first = self.rows == 0;
        match &self.format {
            Format::Tsv { .. } => {
                let part = tsv(columns, rows, false);
                self.line(&part)?;
            }
            Format::Csv { .. } => {
                let part = csv(columns, rows, false);
                self.line(&part)?;
            }
            Format::Delimited(d) => {
                let mut damaged = 0;
                let part = d.lines(columns, rows, &mut damaged);
                self.damaged += damaged;
                self.line(&part)?;
            }
            Format::Json | Format::JsonPretty => {
                let part = json_objects(columns, rows);
                self.put(if first { "[\n" } else { ",\n" })?;
                self.put(&part)?;
            }
            Format::JsonLines => {
                let mut part = String::new();
                for r in rows {
                    part.push_str(&json_object(columns, &self.names, r));
                    part.push('\n');
                }
                self.raw(&part)?;
            }
            Format::Markdown => {
                let part = markdown_rows(rows);
                self.line(&part)?;
            }
            Format::Sql(d, target) => {
                let part = sql_insert(*d, target, columns, rows);
                self.line(&part)?;
            }
            Format::Update(d, target) => {
                let part = sql_update(*d, target, columns, rows);
                self.line(&part)?;
            }
            Format::List => {
                let part = comma_list(rows);
                if !first {
                    self.raw(", ")?;
                }
                self.raw(&part)?;
            }
            Format::Html => {
                let part = html_rows(rows);
                self.raw(&part)?;
            }
            Format::Xml => {
                let part = xml_rows(columns, rows);
                self.raw(&part)?;
            }
        }
        self.rows += rows.len() as u64;
        Ok(())
    }

    fn end(&mut self) -> io::Result<()> {
        match &self.format {
            Format::Json | Format::JsonPretty if self.rows == 0 => self.put("[]")?,
            Format::Json | Format::JsonPretty => self.put("\n]")?,
            Format::Html => self.raw(HTML_END)?,
            Format::Xml => self.raw(XML_END)?,
            _ => {}
        }
        if let Some(ind) = self.indent.as_mut() {
            let mut rest = String::new();
            ind.finish(&mut rest);
            self.raw(&rest)?;
        }
        if self.trailing_newline && self.last.is_some_and(|b| b != b'\n') {
            self.raw("\n")?;
        }
        self.out.flush()
    }

    fn rows_written(&self) -> u64 {
        self.rows
    }

    fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(test)]
mod tests;
