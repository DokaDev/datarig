//! Result rows as text to copy or to write to a file: TSV (what a
//! spreadsheet pastes into cells), CSV (RFC 4180), JSON (an array of objects), a Markdown table
//! and SQL `INSERT` statements; also a comma list, indented
//! JSON, an HTML table, XML, an SQL `IN` list and SQL `UPDATE` statements. Pure functions over the values as the driver
//! delivered them (`None` is SQL NULL); the UI picks the rows and columns. The SQL is written
//! in a [`Dialect`]: its identifier and literal quoting, and its own clauses.
//!
//! NULL and the empty string stay apart wherever the format can tell them apart: CSV writes
//! NULL as an empty field and the empty string as `""`, JSON as `null` and `""`, SQL as `NULL`
//! and `''`; TSV and Markdown show both as an empty cell / `NULL` as a word respectively.

use crate::driver::{ArrayElement, ColumnMeta, ValueKind};
use crate::sql::dialect::Dialect;
use stream::RowSink as _;

/// How a column's values are written where the format has types (JSON, SQL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// A number type: written as a number when the value is one (`NaN` and `Infinity` are
    /// not: they stay strings / literals).
    Number,
    /// `bool`: `true` / `false`.
    Bool,
    /// `json` / `jsonb`: the value itself in JSON, a string literal in SQL.
    Json,
    /// An array (`int4[]`, any number of dimensions): nested JSON arrays of its elements (of
    /// this kind), a string literal (`'{{1,2},{3,4}}'`) in SQL.
    Array(Element),
    /// Binary data, as the text the driver gives (PostgreSQL's `bytea`: `\x0102`). Written as
    /// [`Kind::Text`] is in every format; kept apart so a dialect can write its own literal.
    Bytes,
    /// A bit string, as the text the driver gives (`0101`). Written as [`Kind::Text`] is in every
    /// format; kept apart so a dialect can write its own literal.
    Bit,
}

/// The kind of an array's elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    Text,
    Number,
    Bool,
    Json,
}

impl Kind {
    /// The kind of a result column, from the kind of value the driver says it holds. A kind the
    /// driver does not name ([`ValueKind::Other`]), or a type of the database's own named like
    /// an array, is read from the type's name ([`Kind::of`]), as it always was.
    pub fn of_column(c: &ColumnMeta) -> Kind {
        match c.kind {
            ValueKind::Array(_) => Kind::from(c.kind),
            ValueKind::Other => Kind::of(&c.type_name, c.numeric, c.json),
            // Whatever its kind: a type of the database's own (a domain over a number, say) may
            // be named like an array, and such a name always made it one.
            _ if c.type_name.ends_with("[]") => Kind::of(&c.type_name, c.numeric, c.json),
            k => Kind::from(k),
        }
    }

    /// The kind of a PostgreSQL result column from its type name (as the PostgreSQL driver
    /// names it) and the driver's numeric/json flags. An
    /// array type (`name[]`) gets its elements' kind from the element type's name. `box[]` is
    /// text: its elements are separated by `;`, and each box has commas of its own.
    pub fn of(type_name: &str, numeric: bool, json: bool) -> Kind {
        if let Some(elem) = type_name.strip_suffix("[]")
            && elem != "box"
        {
            return Kind::Array(match elem {
                "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "oid" => Element::Number,
                "bool" => Element::Bool,
                "json" | "jsonb" => Element::Json,
                _ => Element::Text,
            });
        }
        if json {
            Kind::Json
        } else if numeric {
            Kind::Number
        } else if matches!(type_name, "bool" | "boolean") {
            Kind::Bool
        } else {
            Kind::Text
        }
    }
}

impl From<ValueKind> for Kind {
    /// Numbers and booleans are written bare where they can be, JSON embedded, arrays element
    /// by element; bytes and bits as such (text so far), every other kind as text.
    fn from(k: ValueKind) -> Kind {
        match k {
            ValueKind::Integer | ValueKind::Decimal | ValueKind::Float => Kind::Number,
            ValueKind::Bool => Kind::Bool,
            ValueKind::Json => Kind::Json,
            ValueKind::Array(ArrayElement::Number) => Kind::Array(Element::Number),
            ValueKind::Array(ArrayElement::Bool) => Kind::Array(Element::Bool),
            ValueKind::Array(ArrayElement::Json) => Kind::Array(Element::Json),
            ValueKind::Array(ArrayElement::Text) => Kind::Array(Element::Text),
            ValueKind::Bytes => Kind::Bytes,
            ValueKind::Bit => Kind::Bit,
            ValueKind::Text
            | ValueKind::Date
            | ValueKind::Time
            | ValueKind::Timestamp
            | ValueKind::TimestampTz
            | ValueKind::Interval
            | ValueKind::Other => Kind::Text,
        }
    }
}

/// A column to write: its name and kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column<'a> {
    pub name: &'a str,
    pub kind: Kind,
}

/// One row: a value per column, `None` for NULL.
pub type Row<'a> = Vec<Option<&'a str>>;

/// Where SQL `INSERT` statements go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    /// A known table, with the table's own name of each column (not the result's alias).
    /// `overriding`: a `GENERATED ALWAYS AS IDENTITY` column is written, so each statement says
    /// `OVERRIDING SYSTEM VALUE` (PostgreSQL's clause; other dialects have no such column).
    Table { schema: &'a str, name: &'a str, columns: Vec<&'a str>, overriding: bool },
    /// Not known (a join, an expression): a placeholder the user replaces.
    Unknown,
}

/// The placeholder for a table that is not known.
pub const TABLE_PLACEHOLDER: &str = "<table>";

/// Tab-separated values. A value with a tab or a line break, or one that starts with a double
/// quote, is quoted (`"…"`, quotes doubled), the way spreadsheets read pasted cells; any other
/// value is written as it is. NULL is an empty cell.
pub fn tsv(columns: &[Column], rows: &[Row], header: bool) -> String {
    let field = |v: &str| {
        if v.contains(['\t', '\n', '\r']) || v.starts_with('"') {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v.to_string()
        }
    };
    let mut out = Vec::new();
    if header {
        out.push(columns.iter().map(|c| field(c.name)).collect::<Vec<_>>().join("\t"));
    }
    for r in rows {
        out.push(r.iter().map(|v| v.map(field).unwrap_or_default()).collect::<Vec<_>>().join("\t"));
    }
    out.join("\n")
}

/// Comma-separated values (RFC 4180, lines ended by `\n`): a value with a comma, a double
/// quote or a line break is quoted; NULL is an empty field, the empty string `""`.
pub fn csv(columns: &[Column], rows: &[Row], header: bool) -> String {
    let field = |v: &str| {
        if v.is_empty() || v.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v.to_string()
        }
    };
    let mut out = Vec::new();
    if header {
        out.push(columns.iter().map(|c| field(c.name)).collect::<Vec<_>>().join(","));
    }
    for r in rows {
        out.push(r.iter().map(|v| v.map(field).unwrap_or_default()).collect::<Vec<_>>().join(","));
    }
    out.join("\n")
}

/// The keys of a JSON object for `columns`: a name that is used again gets `_2`, `_3`, …
fn unique_names(columns: &[Column]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in columns {
        let mut name = c.name.to_string();
        let mut n = 1;
        while out.contains(&name) {
            n += 1;
            name = format!("{}_{n}", c.name);
        }
        out.push(name);
    }
    out
}

/// A JSON string literal.
fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

/// Whether `s` is a JSON number literal (RFC 8259: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`).
/// It is then written as it is: JSON numbers have no precision limit, so `numeric` and `int8`
/// values keep every digit (never through `f64`). `NaN`, `Infinity`, `-Infinity`, `1,000`,
/// `$1.00` are not numbers.
fn json_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - start
    };
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            digits(&mut i);
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if digits(&mut i) == 0 {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

/// One array element (its text, `None` for NULL) as JSON.
fn json_element(v: Option<&str>, e: Element) -> String {
    let Some(v) = v else { return "null".to_string() };
    match e {
        Element::Number if json_number(v) => v.to_string(),
        Element::Bool if matches!(v, "t" | "true") => "true".to_string(),
        Element::Bool if matches!(v, "f" | "false") => "false".to_string(),
        Element::Json if serde_json::from_str::<serde::de::IgnoredAny>(v).is_ok() => v.trim().to_string(),
        _ => json_string(v),
    }
}

/// A PostgreSQL array literal (as `array_out` writes it: `{{1,2},{3,NULL}}`, elements quoted
/// with `"` and `\` escapes when needed) as nested JSON arrays. `None` when it is not one, or
/// has explicit bounds (`[0:1]={…}`: JSON arrays have no lower bound), so the caller writes
/// the literal as a string.
fn json_array(v: &str, e: Element) -> Option<String> {
    let b = v.as_bytes();
    let mut i = 0;
    let out = json_array_at(v, b, &mut i, e)?;
    (i == b.len()).then_some(out)
}

/// The array starting at `b[*i]` (a `{`); moves `*i` past its `}`.
fn json_array_at(v: &str, b: &[u8], i: &mut usize, e: Element) -> Option<String> {
    if b.get(*i) != Some(&b'{') {
        return None;
    }
    *i += 1;
    let mut items: Vec<String> = Vec::new();
    if b.get(*i) == Some(&b'}') {
        *i += 1;
        return Some("[]".to_string());
    }
    loop {
        match b.get(*i)? {
            b'{' => items.push(json_array_at(v, b, i, e)?),
            b'"' => {
                *i += 1;
                let mut s = Vec::new();
                loop {
                    match b.get(*i)? {
                        b'\\' => {
                            s.push(*b.get(*i + 1)?);
                            *i += 2;
                        }
                        b'"' => {
                            *i += 1;
                            break;
                        }
                        c => {
                            s.push(*c);
                            *i += 1;
                        }
                    }
                }
                items.push(json_element(Some(std::str::from_utf8(&s).ok()?), e));
            }
            _ => {
                let start = *i;
                while !matches!(b.get(*i)?, b',' | b'}') {
                    *i += 1;
                }
                let t = &v[start..*i];
                items.push(json_element(if t.eq_ignore_ascii_case("null") { None } else { Some(t) }, e));
            }
        }
        match b.get(*i)? {
            b',' => *i += 1,
            b'}' => {
                *i += 1;
                return Some(format!("[{}]", items.join(",")));
            }
            _ => return None,
        }
    }
}

/// A JSON array with one object per row (one per line), keys in column order. Numbers and
/// booleans keep their type, a json/jsonb value is embedded as it is (a string if it does not
/// parse), NULL is `null`.
///
/// Numbers (also those inside json/jsonb values) are written with the exact text the database gave, every digit kept (JSON numbers
/// have no precision limit; readers that parse them into doubles may still round). A number
/// column's value that is not a JSON number literal — `NaN`, `Infinity`, `-Infinity` of
/// `numeric` and the floats, a formatted `money` value — is written as a JSON string of that
/// text.
///
/// An array column's value becomes nested JSON arrays (`{{1,2},{3,4}}` -> `[[1,2],[3,4]]`) with
/// its elements typed the same way; an array with explicit bounds (`[0:1]={7,8}`) stays a
/// string of its literal.
pub fn json(columns: &[Column], rows: &[Row]) -> String {
    let objects = json_objects(columns, rows);
    if objects.is_empty() { "[]".to_string() } else { format!("[\n{objects}\n]") }
}

/// A value of column `c` in [`json`].
fn json_value(c: &Column, v: Option<&str>) -> String {
    let Some(v) = v else { return "null".to_string() };
    match c.kind {
        Kind::Number if json_number(v) => v.to_string(),
        Kind::Bool if v == "true" || v == "false" => v.to_string(),
        Kind::Array(e) => json_array(v, e).unwrap_or_else(|| json_string(v)),
        // Embedded as the database wrote it (parsing it into values would round big
        // numbers through f64); a string when it is not JSON.
        Kind::Json if serde_json::from_str::<serde::de::IgnoredAny>(v).is_ok() => v.trim().to_string(),
        _ => json_string(v),
    }
}

/// The objects of [`json`] without the brackets, one per line, joined by `,\n`.
fn json_objects(columns: &[Column], rows: &[Row]) -> String {
    let names = unique_names(columns);
    let objects: Vec<String> = rows.iter().map(|r| format!("  {}", json_object(columns, &names, r))).collect();
    objects.join(",\n")
}

/// One row of [`json`] as an object on one line (`{"id": 1, "name": "x"}`), keys `names`.
fn json_object(columns: &[Column], names: &[String], r: &Row) -> String {
    let fields: Vec<String> = columns
        .iter()
        .zip(names)
        .zip(r)
        .map(|((c, n), v)| format!("{}: {}", json_string(n), json_value(c, *v)))
        .collect();
    format!("{{{}}}", fields.join(", "))
}

/// A Markdown table: `|` escaped, line breaks as `<br>`, NULL as `NULL`, number columns
/// right-aligned.
pub fn markdown(columns: &[Column], rows: &[Row]) -> String {
    let body = markdown_rows(rows);
    if body.is_empty() { markdown_header(columns) } else { format!("{}\n{body}", markdown_header(columns)) }
}

fn md_cell(v: &str) -> String {
    v.replace('\\', "\\\\").replace('|', "\\|").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>")
}

fn md_line(cells: Vec<String>) -> String {
    format!("| {} |", cells.join(" | "))
}

/// The two header lines of [`markdown`].
fn markdown_header(columns: &[Column]) -> String {
    let names = md_line(columns.iter().map(|c| md_cell(c.name)).collect());
    let rule =
        md_line(columns.iter().map(|c| if c.kind == Kind::Number { "---:" } else { "---" }.to_string()).collect());
    format!("{names}\n{rule}")
}

/// The row lines of [`markdown`].
fn markdown_rows(rows: &[Row]) -> String {
    rows.iter()
        .map(|r| md_line(r.iter().map(|v| v.map_or_else(|| "NULL".to_string(), md_cell)).collect()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A PostgreSQL identifier, always quoted (`"` doubled): any name, any case, keywords.
/// [`Dialect::force_quote_ident`] of [`Dialect::Postgres`], for the PostgreSQL-only renderers.
pub fn quote_ident(s: &str) -> String {
    Dialect::Postgres.force_quote_ident(s)
}

/// A PostgreSQL string literal (`'` doubled). Written for `standard_conforming_strings = on`
/// (the default since 9.1), where a backslash is an ordinary character.
/// [`Dialect::quote_literal`] of [`Dialect::Postgres`], for the PostgreSQL-only renderers.
pub fn quote_literal(s: &str) -> String {
    Dialect::Postgres.quote_literal(s)
}

/// Whether `s` is a plain SQL number literal (`12`, `-3.5`, `1e-3`).
fn sql_number(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exp) = match t.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e.strip_prefix(['+', '-']).unwrap_or(e))),
        None => (t, None),
    };
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        Some((a, b)) => (digits(a) || a.is_empty()) && (digits(b) || b.is_empty()) && !(a.is_empty() && b.is_empty()),
        None => digits(mantissa),
    };
    mantissa_ok && exp.is_none_or(digits)
}

/// Whether the number `s` is a negative zero (`-0`, `-0.0e5`).
fn negative_zero(s: &str) -> bool {
    s.strip_prefix('-')
        .is_some_and(|m| m.split(['e', 'E']).next().is_some_and(|m| m.bytes().all(|b| matches!(b, b'0' | b'.'))))
}

/// One `INSERT` statement per row in dialect `d`, `;`-terminated, one per line. Numbers and
/// booleans are written bare; every other value (text, json, arrays `{…}`, bytea `\x…`, dates)
/// as a string literal the server converts to the column's type; NULL as `NULL`. Identifiers
/// are always quoted. An unknown table is written as [`TABLE_PLACEHOLDER`] with the result's
/// column names.
pub fn sql_insert(d: Dialect, target: &Target, columns: &[Column], rows: &[Row]) -> String {
    let (table, names, overriding): (String, Vec<&str>, bool) = match target {
        Target::Table { schema, name, columns: cols, overriding } => {
            (format!("{}.{}", d.force_quote_ident(schema), d.force_quote_ident(name)), cols.clone(), *overriding)
        }
        Target::Unknown => (TABLE_PLACEHOLDER.to_string(), columns.iter().map(|c| c.name).collect(), false),
    };
    let overriding = match d {
        Dialect::Postgres if overriding => " OVERRIDING SYSTEM VALUE",
        Dialect::Postgres | Dialect::MySql(_) => "",
    };
    let list = names.iter().map(|n| d.force_quote_ident(n)).collect::<Vec<_>>().join(", ");
    let value = |c: &Column, v: Option<&str>| sql_value(d, c.kind, v);
    rows.iter()
        .map(|r| {
            let values = columns.iter().zip(r).map(|(c, v)| value(c, *v)).collect::<Vec<_>>().join(", ");
            format!("INSERT INTO {table} ({list}){overriding} VALUES ({values});")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The values of every cell, row by row and left to right within a row, joined by `, `
/// ("comma list"). Values are written as they are, NULL as `NULL`. Several columns are
/// flattened the same way: `a1, b1, a2, b2`.
pub fn comma_list(rows: &[Row]) -> String {
    rows.iter().flat_map(|r| r.iter().map(|v| v.unwrap_or("NULL"))).collect::<Vec<_>>().join(", ")
}

/// [`json`] indented: each object and array on lines of its own, two spaces a level. The text
/// of every value is kept (numbers keep every digit, embedded json/jsonb values keep theirs),
/// only white space between tokens changes.
pub fn json_pretty(columns: &[Column], rows: &[Row]) -> String {
    indent_json(&json(columns, rows))
}

/// Indent JSON text `s` (any valid JSON), keeping every token as it is.
fn indent_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    let mut depth = 0usize;
    let (mut in_str, mut escaped) = (false, false);
    let chars: Vec<char> = s.chars().collect();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                // An empty object or array stays on its line.
                let next = chars[i..].iter().position(|c| !c.is_whitespace()).map(|k| chars[i + k]);
                if matches!(next, Some('}' | ']')) {
                    let close = chars[i..].iter().position(|c| !c.is_whitespace()).unwrap_or(0);
                    out.push(chars[i + close]);
                    i += close + 1;
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}

/// Text for HTML: `&`, `<`, `>`, `"` and `'` as entities, line breaks as `<br>`. Control
/// characters other than tab (C0, DEL and C1) and the noncharacters U+FFFE/U+FFFF, which HTML
/// does not allow even as references, become U+FFFD, as in [`xml_text`].
pub fn html_text(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("<br>");
            }
            '\n' => out.push_str("<br>"),
            '\t' => out.push(c),
            c if c.is_control() || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

/// A plain HTML table: the column names in `<thead>`, one `<tr>` per row in `<tbody>`, every
/// value escaped ([`html_text`]), NULL as `NULL` (as the grid shows it), no styles or classes.
pub fn html(columns: &[Column], rows: &[Row]) -> String {
    let mut out = html_head(columns);
    out.push_str(&html_rows(rows));
    out.push_str(HTML_END);
    out
}

const HTML_END: &str = "  </tbody>\n</table>";

fn html_head(columns: &[Column]) -> String {
    let names: String = columns.iter().map(|c| format!("<th>{}</th>", html_text(c.name))).collect();
    format!("<table>\n  <thead>\n    <tr>{names}</tr>\n  </thead>\n  <tbody>\n")
}

fn html_rows(rows: &[Row]) -> String {
    rows.iter()
        .map(|r| {
            let cells: String = r.iter().map(|v| format!("<td>{}</td>", html_text(v.unwrap_or("NULL")))).collect();
            format!("    <tr>{cells}</tr>\n")
        })
        .collect()
}

/// Text for XML (1.0): `&`, `<`, `>`, `"` and `'` as entities, a carriage return as `&#13;` (a
/// parser would turn a bare one into a line feed). Characters XML 1.0 cannot hold at all (the
/// control characters other than tab, line feed and carriage return, and U+FFFE/U+FFFF) become
/// U+FFFD.
pub fn xml_text(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

/// XML with a simple schema:
///
/// ```xml
/// <?xml version="1.0" encoding="UTF-8"?>
/// <rows>
///   <row>
///     <column name="id">1</column>
///     <column name="note" null="true"/>
///   </row>
/// </rows>
/// ```
///
/// One `<row>` per row, one `<column>` per cell in column order with the column's name in
/// `name`; NULL is an empty element with `null="true"` (the empty string is an empty element
/// without it). Names and values are escaped ([`xml_text`]).
pub fn xml(columns: &[Column], rows: &[Row]) -> String {
    format!("{XML_HEAD}{}{XML_END}", xml_rows(columns, rows))
}

const XML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rows>\n";
const XML_END: &str = "</rows>";

fn xml_rows(columns: &[Column], rows: &[Row]) -> String {
    rows.iter()
        .map(|r| {
            let cells: String = columns
                .iter()
                .zip(r)
                .map(|(c, v)| match v {
                    Some(v) => format!("    <column name=\"{}\">{}</column>\n", xml_text(c.name), xml_text(v)),
                    None => format!("    <column name=\"{}\" null=\"true\"/>\n", xml_text(c.name)),
                })
                .collect();
            format!("  <row>\n{cells}  </row>\n")
        })
        .collect()
}

/// A value as an SQL literal of dialect `d`: NULL, a number or a boolean bare, anything else a
/// string literal the server converts to the column's type (as in [`sql_insert`]).
pub fn sql_value(d: Dialect, kind: Kind, v: Option<&str>) -> String {
    match v {
        None => "NULL".to_string(),
        // A negative zero (`-0` of a float) is quoted: bare, it is the integer 0 and loses its sign.
        Some(v) if kind == Kind::Number && sql_number(v) && !negative_zero(v) => v.to_string(),
        Some(v) if kind == Kind::Bool && (v == "true" || v == "false") => v.to_string(),
        // MySQL shows bytes as `0x…` and bits as `b'…'` (every byte, every bit): written as its
        // hexadecimal and bit literals, which are the value itself (a string literal of that
        // text would be the text).
        Some(v) if matches!(d, Dialect::MySql(_)) && kind == Kind::Bytes && mysql_hex(v) => {
            format!("X'{}'", &v[2..])
        }
        Some(v) if matches!(d, Dialect::MySql(_)) && kind == Kind::Bit && mysql_bits(v) => v.to_string(),
        // The same string whatever the session's `NO_BACKSLASH_ESCAPES`.
        Some(v) if matches!(d, Dialect::MySql(_)) => mysql_string(v),
        // Bytes (`\x…`) and bits too: PostgreSQL reads them from a string literal.
        Some(v) => d.quote_literal(v),
    }
}

/// `0x` and an even number of upper-case hexadecimal digits: binary data as the MySQL driver
/// shows it.
fn mysql_hex(v: &str) -> bool {
    v.strip_prefix("0x").is_some_and(|h| h.len() % 2 == 0 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F')))
}

/// `b'…'` of at least one binary digit: a bit value as the MySQL driver shows it.
fn mysql_bits(v: &str) -> bool {
    v.strip_prefix("b'")
        .and_then(|r| r.strip_suffix('\''))
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b == b'0' || b == b'1'))
}

/// `s` as a MySQL string literal that reads the same whatever the session's sql mode: as it is
/// (quotes doubled) unless it holds a backslash or a NUL, which `NO_BACKSLASH_ESCAPES` reads
/// otherwise; such a string is written as its UTF-8 bytes, `_utf8mb4 X'…'`.
fn mysql_string(s: &str) -> String {
    if s.contains(['\\', '\0']) {
        let hex: String = s.bytes().map(|b| format!("{b:02X}")).collect();
        format!("_utf8mb4 X'{hex}'")
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

/// Why rows are not copied as SQL: a value of a column would not be written whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotWhole {
    /// MySQL's `FLOAT`: the server sends it rounded to about six digits.
    Float,
    /// Text read in a character set that is not UTF-8 (`character_set_results`, here its
    /// name): what the session shows is not the text.
    Charset(String),
    /// A `TIMESTAMP` shown in a time zone that is not a fixed offset (here its name): a time in
    /// the hour a clock goes back is two instants.
    TimeZone(String),
    /// An `AUTO_INCREMENT` column holds 0, which an `INSERT` takes as "the next number".
    AutoIncrementZero,
    /// A value not in the form the driver shows its kind in (bytes, bits).
    NotShownWhole,
    /// A `TIMESTAMP` or a text read before the session said its time zone and character set.
    SessionUnknown,
    /// A `TIMESTAMP` or a text whose pages were read in different time zones or character sets.
    SessionChanged,
}

/// A column a copy as SQL refuses, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub column: String,
    pub why: NotWhole,
}

/// A result column to copy as SQL: its name, its type and kind as the driver gives them, and
/// whether it is an `AUTO_INCREMENT` column of the table the rows go into.
#[derive(Clone, Copy, Debug)]
pub struct SqlColumn<'a> {
    pub name: &'a str,
    pub type_name: &'a str,
    pub kind: crate::driver::ValueKind,
    pub auto_increment: bool,
}

/// How each column's values are written as SQL ([`sql_cells`]).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Rule {
    /// As [`sql_value`] writes it (every PostgreSQL value).
    Plain,
    /// MySQL bytes or bits: in the driver's form.
    Shown,
    /// A MySQL `TIMESTAMP`, with the offset of the session's zone (`+00:00`) after it.
    Timestamp(String),
    /// MySQL text that is not copied, why: refused for any value.
    Refuse(NotWhole),
    /// An `AUTO_INCREMENT` column: 0 is refused.
    AutoIncrement,
}

/// The rules a copy as SQL writes each column of a result by ([`sql_cells`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlCells {
    names: Vec<String>,
    rules: Vec<Rule>,
}

/// The fixed offset from UTC of time zone `zone` (`+05:30`, `UTC`), written `+HH:MM`; `None`
/// for a zone with daylight saving or that is not known. `SYSTEM` is the server's own zone
/// `server_zone`.
pub fn fixed_offset(zone: &str, server_zone: Option<&str>) -> Option<String> {
    let zone = if zone.eq_ignore_ascii_case("SYSTEM") { server_zone? } else { zone };
    if ["UTC", "GMT", "Etc/UTC", "Etc/GMT", "Universal", "Zulu"].iter().any(|z| zone.eq_ignore_ascii_case(z)) {
        return Some("+00:00".to_string());
    }
    let (sign, rest) = match zone.as_bytes().first() {
        Some(b'+') => ('+', &zone[1..]),
        Some(b'-') => ('-', &zone[1..]),
        _ => return None,
    };
    let (h, m) = rest.split_once(':')?;
    let digits = |s: &str| (1..=2).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(h) || m.len() != 2 || !digits(m) {
        return None;
    }
    Some(format!("{sign}{:0>2}:{m}", h))
}

/// How the columns `columns` of a result are copied as SQL in dialect `d`, or the first one that
/// cannot be. On MySQL: a `FLOAT` is refused (the server sends it rounded); text read in a
/// character set that is not UTF-8 is refused; a `TIMESTAMP` is written with the offset of the
/// time zone it was read in when that is fixed ([`fixed_offset`]: `read_in` the session's values
/// when the result was read, `server_zone` the server's own zone), and refused otherwise; a
/// `TIMESTAMP` or a text whose values are not known, or changed between the result's pages, is
/// refused; bytes and bits must be in the driver's form; an `AUTO_INCREMENT` value of 0 is
/// refused (checked per value, [`SqlCells::cell`]). PostgreSQL's values are written as they are.
pub fn sql_cells(
    d: Dialect,
    columns: &[SqlColumn],
    read_in: &crate::driver::ReadIn,
    server_zone: Option<&str>,
) -> Result<SqlCells, Refused> {
    use crate::driver::ReadIn;
    use crate::driver::ValueKind as V;
    let names = columns.iter().map(|c| c.name.to_string()).collect();
    if d == Dialect::Postgres {
        return Ok(SqlCells { names, rules: vec![Rule::Plain; columns.len()] });
    }
    let refused = |c: &SqlColumn, why| Refused { column: c.name.to_string(), why };
    // Only `utf8mb4` is every character (`utf8mb3` turns an emoji into `?`).
    let utf8 = |cs: &str| cs.eq_ignore_ascii_case("utf8mb4");
    let mut rules = Vec::with_capacity(columns.len());
    for c in columns {
        let rule = match c.kind {
            V::Float if c.type_name.starts_with("float") => return Err(refused(c, NotWhole::Float)),
            V::Bytes | V::Bit => Rule::Shown,
            V::TimestampTz => match read_in {
                ReadIn::Values(s) => match fixed_offset(&s.time_zone, server_zone) {
                    Some(offset) => Rule::Timestamp(offset),
                    None => return Err(refused(c, NotWhole::TimeZone(s.time_zone.clone()))),
                },
                ReadIn::Unknown => return Err(refused(c, NotWhole::SessionUnknown)),
                ReadIn::Changed => return Err(refused(c, NotWhole::SessionChanged)),
            },
            V::Text | V::Json | V::Other => match read_in {
                ReadIn::Values(s) if !utf8(&s.charset) => Rule::Refuse(NotWhole::Charset(s.charset.clone())),
                ReadIn::Unknown => Rule::Refuse(NotWhole::SessionUnknown),
                ReadIn::Changed => Rule::Refuse(NotWhole::SessionChanged),
                _ if c.auto_increment => Rule::AutoIncrement,
                _ => Rule::Plain,
            },
            _ if c.auto_increment => Rule::AutoIncrement,
            _ => Rule::Plain,
        };
        rules.push(rule);
    }
    Ok(SqlCells { names, rules })
}

impl SqlCells {
    /// Value `v` of column `i` as the writer takes it (a `TIMESTAMP` with its offset), or why it
    /// is not copied ([`sql_cells`]).
    pub fn cell<'a>(&self, i: usize, v: &'a str) -> Result<std::borrow::Cow<'a, str>, Refused> {
        let refused = |why| Refused { column: self.names.get(i).cloned().unwrap_or_default(), why };
        match self.rules.get(i).unwrap_or(&Rule::Plain) {
            Rule::Plain => Ok(v.into()),
            Rule::Shown if mysql_hex(v) || mysql_bits(v) => Ok(v.into()),
            Rule::Shown => Err(refused(NotWhole::NotShownWhole)),
            // A zero timestamp is no instant: it has no offset.
            Rule::Timestamp(_) if v.starts_with("0000-00-00") => Ok(v.into()),
            Rule::Timestamp(offset) => Ok(format!("{v}{offset}").into()),
            Rule::Refuse(why) => Err(refused(why.clone())),
            Rule::AutoIncrement if v == "0" => Err(refused(NotWhole::AutoIncrementZero)),
            Rule::AutoIncrement => Ok(v.into()),
        }
    }
}

/// Why values are not copied as an SQL `IN` list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotInList {
    /// More than one column: an `IN` list is of one column's values.
    SeveralColumns,
    /// No value to list (no rows, or only NULLs).
    NoValues,
}

/// One column's values as an SQL `IN` list, `('a', 'b', 3)`, literals as [`sql_value`] writes
/// them, in order, each value once. NULL is left out (`x IN (…, NULL)` never matches a NULL and
/// makes `NOT IN` match nothing).
pub fn sql_in(d: Dialect, columns: &[Column], rows: &[Row]) -> Result<String, NotInList> {
    let [column] = columns else { return Err(NotInList::SeveralColumns) };
    let mut seen = std::collections::HashSet::new();
    let values: Vec<String> = rows
        .iter()
        .filter_map(|r| r.first().copied().flatten())
        .filter(|v| seen.insert(*v))
        .map(|v| sql_value(d, column.kind, Some(v)))
        .collect();
    if values.is_empty() {
        return Err(NotInList::NoValues);
    }
    Ok(format!("({})", values.join(", ")))
}

/// Where SQL `UPDATE` statements go: the table, the result columns to set (their index in the
/// row and their name in the table) and the primary key columns that find the row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateTarget<'a> {
    pub schema: &'a str,
    pub name: &'a str,
    pub set: Vec<(usize, &'a str)>,
    pub keys: Vec<(usize, &'a str)>,
}

/// One `UPDATE` statement per row in dialect `d`, `;`-terminated, one per line:
/// `UPDATE "s"."t" SET "a" = 1, "b" = 'x' WHERE "id" = 5 AND "k" = 'z';`. Values are literals as
/// in [`sql_value`]; a NULL is set as `NULL`. The key columns are a primary key, so they are
/// never NULL in a table's rows (a NULL there would be written `= NULL`, which matches no row).
pub fn sql_update(d: Dialect, target: &UpdateTarget, columns: &[Column], rows: &[Row]) -> String {
    let table = format!("{}.{}", d.force_quote_ident(target.schema), d.force_quote_ident(target.name));
    let part = |r: &Row, (i, name): &(usize, &str)| {
        format!("{} = {}", d.force_quote_ident(name), sql_value(d, columns[*i].kind, r[*i]))
    };
    rows.iter()
        .map(|r| {
            let set = target.set.iter().map(|c| part(r, c)).collect::<Vec<_>>().join(", ");
            let key = target.keys.iter().map(|c| part(r, c)).collect::<Vec<_>>().join(" AND ");
            format!("UPDATE {table} SET {set} WHERE {key};")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A format of [`Writer`] and [`stream::TextWriter`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Format<'a> {
    Tsv {
        header: bool,
    },
    Csv {
        header: bool,
    },
    Json,
    /// [`json`], indented.
    JsonPretty,
    Markdown,
    /// [`sql_insert`] in a dialect.
    Sql(Dialect, Target<'a>),
    /// [`comma_list`].
    List,
    Html,
    Xml,
    /// [`sql_update`] in a dialect.
    Update(Dialect, UpdateTarget<'a>),
    /// Delimited text with the options of a file export ([`stream::Delimited`]): CSV with
    /// another delimiter, another quoting, another text for NULL.
    Delimited(stream::Delimited),
    /// One JSON object per line (JSON Lines), the objects of [`json`] without the array.
    JsonLines,
}

/// The same text as the functions above, written a chunk of rows at a time, so a large copy
/// never holds every row at once (the rows of a spilled result are read from disk chunk by
/// chunk). The text of any split into chunks is the text of all rows at once. It is a
/// [`stream::TextWriter`] into memory.
pub struct Writer<'a> {
    inner: stream::TextWriter<'a, Vec<u8>>,
}

impl<'a> Writer<'a> {
    pub fn new(format: Format<'a>, columns: &'a [Column<'a>]) -> Self {
        let mut inner = stream::TextWriter::new(format, columns, Vec::new());
        // Writing to memory does not fail.
        let _ = inner.begin();
        Self { inner }
    }

    /// Rows written so far.
    pub fn count(&self) -> usize {
        usize::try_from(self.inner.rows_written()).unwrap_or(usize::MAX)
    }

    /// Write the next rows.
    pub fn rows(&mut self, rows: &[Row]) {
        let _ = self.inner.rows(rows);
    }

    pub fn finish(mut self) -> String {
        let _ = self.inner.end();
        // Every piece written is a `&str`.
        String::from_utf8(self.inner.into_inner())
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}

pub mod keyset;
pub mod stream;

#[cfg(test)]
mod tests;
