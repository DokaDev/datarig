//! A result's rows as a chart: which columns to draw and how, and the numbers to draw, read
//! only from the rows a result already holds (nothing is asked of the server).
//!
//! **Columns.** Each column of a result has a [`Role`]: a number (the driver says it is
//! numeric), a time (its type is a date, a time or a timestamp, or every value of a text column
//! reads as a date), a category (any other text) or unusable (JSON, arrays). Nothing here is
//! specific to one database engine: the roles come from what any driver says about its
//! columns and from their values.
//!
//! **What to draw** ([`Spec`]): a kind of chart ([`Kind`]), the column along the X axis (or the
//! row number), the numeric columns drawn as series (Y) and a column that splits one Y into a
//! series per value ("by"). [`infer`] picks them the way a person would at first sight:
//! a time and numbers → lines over time; a category and numbers → bars (horizontal ones when
//! the labels are long or many); numbers only → lines over the first one.
//!
//! **The numbers** ([`Builder`] → [`Model`]): rows with the same X are summed (a result of a
//! `GROUP BY` has one row per X anyway), NULLs and values that are not numbers are skipped and
//! counted, points along a number or time axis are sorted by it, and too many bars or series
//! keep the largest ones and sum the rest into "others".

pub mod scale;
pub mod time;

use crate::driver::{Cell, ColumnMeta, ValueKind};
use std::collections::HashMap;

/// At most this many series are drawn: past it, the largest ones and an "others" series.
pub const MAX_SERIES: usize = 6;
/// At most this many bars (categories): past it, the largest ones and an "others" bar.
pub const MAX_BARS: usize = 50;
/// More categories than this (or longer labels) make bars horizontal.
const HBAR_CATEGORIES: usize = 12;
const HBAR_LABEL: usize = 12;
/// A column splits the values into series by itself when it has at most this many values.
const AUTO_BY: usize = 10;
/// Rows read to tell the roles and the kind of chart.
pub const SAMPLE: usize = 500;

/// A kind of chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Vertical bars, one group per X value, a bar per series.
    Bar,
    /// Horizontal bars: one line per X value (long or many labels read better).
    HBar,
    /// Lines through the points, in X order.
    Line,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Bar, Kind::HBar, Kind::Line];

    /// The kind `delta` places away, around the ends.
    pub fn step(self, delta: isize) -> Kind {
        let at = Self::ALL.iter().position(|k| *k == self).unwrap_or(0) as isize;
        Self::ALL[(at + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    pub fn bars(self) -> bool {
        self != Kind::Line
    }
}

/// What a time column holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimeKind {
    Date,
    DateTime,
    /// A time of day alone.
    Time,
}

/// What a column can be in a chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Number,
    Time(TimeKind),
    Category,
    /// JSON, an array: not drawn.
    Unusable,
}

impl Role {
    /// The column can go along the X axis.
    pub fn x(self) -> bool {
        self != Role::Unusable
    }

    /// The column can be a series (its values are numbers).
    pub fn y(self) -> bool {
        self == Role::Number
    }

    /// The column can split the values into series.
    pub fn by(self) -> bool {
        self != Role::Unusable
    }
}

/// What the X axis places points by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// The values' text, in the order they first come.
    Category,
    Number,
    Time(TimeKind),
    /// The row's number in the result.
    Row,
}

/// What a chart draws.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Spec {
    pub kind: Kind,
    /// The column along the X axis; `None`: the row number.
    pub x: Option<usize>,
    /// The columns drawn as series (numbers), at most [`MAX_SERIES`].
    pub ys: Vec<usize>,
    /// The column whose values split the first Y into series.
    pub by: Option<usize>,
    /// The value axis is logarithmic (values of zero or less are left out).
    pub log: bool,
}

/// Why a result has no chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsuitable {
    NoRows,
    /// No column holds numbers.
    NoNumber,
    /// No column is chosen as a series.
    NoSeries,
    /// Every value to draw is NULL or not a number (or the X of every row is).
    NoValues,
    /// One point only: nothing to compare.
    OnePoint,
}

/// What was left out, counted (a footnote says it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    /// Rows whose X is NULL.
    pub null_x: u64,
    /// Rows whose X does not read as the axis' number or time.
    pub bad_x: u64,
    /// Values that are NULL.
    pub null_y: u64,
    /// Values that are not numbers (or not finite).
    pub bad_y: u64,
    /// Rows whose "by" value is NULL.
    pub null_by: u64,
}

impl Skipped {
    pub fn nulls(&self) -> u64 {
        self.null_x + self.null_y + self.null_by
    }

    pub fn not_numbers(&self) -> u64 {
        self.bad_x + self.bad_y
    }
}

/// A place along the X axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// The X value's text (the row number along [`Axis::Row`]); empty for "others".
    pub label: String,
    /// Where it is: the number or the time (seconds), the row number, or its index among the
    /// categories.
    pub x: f64,
    /// The first row it comes from (`None` for "others").
    pub first_row: Option<usize>,
    /// How many rows it sums.
    pub rows: usize,
    /// The "others" bar: the categories past [`MAX_BARS`], summed.
    pub others: bool,
}

/// A series: a value per point.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// The Y column's name, or the "by" value (empty for "others").
    pub name: String,
    pub others: bool,
    /// `None`: no value at that point (every value NULL or not a number, or no row).
    pub values: Vec<Option<f64>>,
    /// How many values each point's value sums, and the row of the first one.
    pub rows: Vec<u32>,
    pub first: Vec<Option<usize>>,
    /// The series has a row at each point (its value may still be NULL or not a number).
    pub seen: Vec<bool>,
}

impl Series {
    fn new(name: String, others: bool, points: usize) -> Series {
        Series {
            name,
            others,
            values: vec![None; points],
            rows: vec![0; points],
            first: vec![None; points],
            seen: vec![false; points],
        }
    }

    /// Only the points at `order`, in that order.
    fn pick(&mut self, order: &[usize]) {
        self.values = order.iter().map(|&i| self.values[i]).collect();
        self.rows = order.iter().map(|&i| self.rows[i]).collect();
        self.first = order.iter().map(|&i| self.first[i]).collect();
        self.seen = order.iter().map(|&i| self.seen[i]).collect();
    }

    /// Add a value of `rows` rows to point `i`'s (the first row of the sum is no longer one
    /// row's). Whether it was left out: the sum would pass the largest number.
    fn absorb(&mut self, i: usize, v: Option<f64>, rows: u32, seen: bool) -> bool {
        self.first[i] = None;
        self.seen[i] |= seen;
        let Some(v) = v else { return false };
        let sum = self.values[i].unwrap_or(0.0) + v;
        if !sum.is_finite() {
            return true;
        }
        self.values[i] = Some(sum);
        self.rows[i] += rows;
        false
    }
}

/// The numbers of a chart.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// Bars (any direction: the model is the same) or a line.
    pub kind: Kind,
    pub axis: Axis,
    pub points: Vec<Point>,
    pub series: Vec<Series>,
    pub skipped: Skipped,
    /// The rows read.
    pub rows: usize,
    /// Some point sums more than one row.
    pub merged: bool,
    /// Categories summed in the "others" bar, and series in the "others" series.
    pub other_points: usize,
    pub other_series: usize,
    /// Values of zero or less (a logarithmic axis leaves them out).
    pub nonpositive: usize,
    /// The series come from a column's values: a series has no row at a point of another one
    /// (its line goes on past it).
    pub split: bool,
    /// The smallest and largest value, and of the positive ones.
    span: Option<(f64, f64)>,
    positive: Option<(f64, f64)>,
}

impl Model {
    /// The smallest and the largest value drawn (`log`: of the positive ones).
    pub fn range(&self, log: bool) -> Option<(f64, f64)> {
        if log { self.positive } else { self.span }
    }

    /// The first and the last point's place (points are in X order).
    pub fn x_range(&self) -> (f64, f64) {
        match (self.points.first(), self.points.last()) {
            (Some(a), Some(b)) => (a.x, b.x),
            _ => (0.0, 0.0),
        }
    }

    /// The chart's numbers as TSV: a header (`x` and each series' name), then a line per point;
    /// `others` names the "others" bar and series, an empty cell is no value.
    pub fn tsv(&self, x: &str, others: &str) -> String {
        let names: Vec<&str> = std::iter::once(x)
            .chain(self.series.iter().map(|s| if s.others { others } else { s.name.as_str() }))
            .collect();
        let columns: Vec<crate::export::Column> =
            names.iter().map(|n| crate::export::Column { name: n, kind: crate::export::Kind::Text }).collect();
        let values: Vec<Vec<Option<String>>> = (0..self.points.len())
            .map(|i| self.series.iter().map(|s| s.values[i].map(scale::plain)).collect())
            .collect();
        let rows: Vec<crate::export::Row> = self
            .points
            .iter()
            .zip(&values)
            .map(|(p, v)| {
                std::iter::once(Some(if p.others { others } else { p.label.as_str() }))
                    .chain(v.iter().map(|v| v.as_deref()))
                    .collect()
            })
            .collect();
        crate::export::tsv(&columns, &rows, true)
    }
}

/// The role of each column, from what the driver says and the `sample` rows.
pub fn roles(columns: &[ColumnMeta], sample: &[Vec<Cell>]) -> Vec<Role> {
    columns.iter().enumerate().map(|(i, c)| role(c, i, sample)).collect()
}

fn role(c: &ColumnMeta, i: usize, sample: &[Vec<Cell>]) -> Role {
    let known = match c.kind {
        ValueKind::Json | ValueKind::Array(_) => Some(Role::Unusable),
        // Whatever its kind: a type of the database's own (a domain over a number, say) may be
        // named like an array, and such a name always made it unusable.
        _ if c.type_name.ends_with("[]") => Some(Role::Unusable),
        k if k.is_number() => Some(Role::Number),
        ValueKind::Timestamp | ValueKind::TimestampTz => Some(Role::Time(TimeKind::DateTime)),
        ValueKind::Date => Some(Role::Time(TimeKind::Date)),
        ValueKind::Time => Some(Role::Time(TimeKind::Time)),
        ValueKind::Other => role_by_name(&c.type_name),
        _ => None,
    };
    if let Some(r) = known {
        return r;
    }
    // Text that holds dates (a driver that types them as text, a `to_char`).
    let mut seen = false;
    let mut with_time = false;
    for v in sample.iter().filter_map(|r| r.get(i)?.as_deref()) {
        match time::parse(v) {
            Some(p) if p.date => {
                seen = true;
                with_time |= p.time;
            }
            _ => return Role::Category,
        }
    }
    match (seen, with_time) {
        (false, _) => Role::Category,
        (true, false) => Role::Time(TimeKind::Date),
        (true, true) => Role::Time(TimeKind::DateTime),
    }
}

/// The role a type of a kind the driver does not name (a database's own type, a domain) has
/// by its name: one named like a timestamp, a date or a time is taken as one.
fn role_by_name(type_name: &str) -> Option<Role> {
    let t = type_name.to_ascii_lowercase();
    if t.contains("timestamp") || t.contains("datetime") {
        Some(Role::Time(TimeKind::DateTime))
    } else if t == "date" {
        Some(Role::Time(TimeKind::Date))
    } else if t == "time" || t == "timetz" || t.starts_with("time ") {
        Some(Role::Time(TimeKind::Time))
    } else {
        None
    }
}

/// What to draw at first sight of a result of `rows` rows (`sample`: its first ones).
pub fn infer(roles: &[Role], sample: &[Vec<Cell>], rows: usize) -> Result<Spec, Unsuitable> {
    if rows == 0 {
        return Err(Unsuitable::NoRows);
    }
    let numbers: Vec<usize> = (0..roles.len()).filter(|&i| roles[i].y()).collect();
    if numbers.is_empty() {
        return Err(Unsuitable::NoNumber);
    }
    let time = (0..roles.len()).find(|&i| matches!(roles[i], Role::Time(_)));
    let categories: Vec<usize> = (0..roles.len()).filter(|&i| roles[i] == Role::Category).collect();
    let ys = |skip: Option<usize>| numbers.iter().copied().filter(|&i| Some(i) != skip).take(MAX_SERIES).collect();
    // A column that splits one series into a few.
    let by = |x: usize, ys: &Vec<usize>| {
        (ys.len() == 1)
            .then(|| categories.iter().copied().find(|&c| c != x && (2..=AUTO_BY).contains(&distinct(sample, c))))
            .flatten()
    };
    // One row: its numbers side by side.
    if rows == 1 {
        let x = categories.first().copied().or(time);
        return Ok(Spec { kind: Kind::Bar, x, ys: ys(None), by: None, log: false });
    }
    if let Some(x) = time {
        let ys = ys(None);
        let by = by(x, &ys);
        return Ok(Spec { kind: Kind::Line, x: Some(x), ys, by, log: false });
    }
    if let Some(&x) = categories.first() {
        let ys = ys(None);
        let by = by(x, &ys);
        let longest = sample.iter().filter_map(|r| r.get(x)?.as_deref()).map(|v| v.chars().count()).max();
        let kind = if distinct(sample, x) > HBAR_CATEGORIES || longest.unwrap_or(0) > HBAR_LABEL {
            Kind::HBar
        } else {
            Kind::Bar
        };
        return Ok(Spec { kind, x: Some(x), ys, by, log: false });
    }
    if numbers.len() >= 2 {
        return Ok(Spec { kind: Kind::Line, x: Some(numbers[0]), ys: ys(Some(numbers[0])), by: None, log: false });
    }
    let kind = if rows <= 40 { Kind::Bar } else { Kind::Line };
    Ok(Spec { kind, x: None, ys: numbers, by: None, log: false })
}

/// Different non-NULL values of column `c` in `sample`.
fn distinct(sample: &[Vec<Cell>], c: usize) -> usize {
    let mut seen = std::collections::HashSet::new();
    for v in sample.iter().filter_map(|r| r.get(c)?.as_deref()) {
        seen.insert(v);
    }
    seen.len()
}

/// `spec`, made for columns named `from`, for a result whose columns are `to` with `roles`:
/// the same columns by name, while each still fits its place (a query run again with the same
/// columns keeps what the user chose).
pub fn carry(spec: &Spec, from: &[String], to: &[String], roles: &[Role]) -> Option<Spec> {
    let find = |i: usize| {
        let name = from.get(i)?;
        let mut hits = to.iter().enumerate().filter(|(_, n)| *n == name);
        let (j, _) = hits.next()?;
        // A name twice is not one column any more.
        hits.next().is_none().then_some(j)
    };
    let x = match spec.x {
        Some(i) => Some(find(i).filter(|&j| roles[j].x())?),
        None => None,
    };
    let ys = spec.ys.iter().map(|&i| find(i).filter(|&j| roles[j].y())).collect::<Option<Vec<_>>>()?;
    let by = match spec.by {
        Some(i) => Some(find(i).filter(|&j| roles[j].by())?),
        None => None,
    };
    Some(Spec { kind: spec.kind, x, ys, by, log: spec.log })
}

/// The number in a cell: plain, else with a currency's marks and digit grouping left out
/// (`$1,234.50`, `-€3.00`).
pub fn number(text: &str) -> Option<f64> {
    let t = text.trim();
    if let Ok(v) = t.parse::<f64>() {
        return v.is_finite().then_some(v);
    }
    let (neg, rest) = match t.strip_prefix(['-', '\u{2212}']) {
        Some(r) => (true, r),
        None if t.len() > 2 && t.starts_with('(') && t.ends_with(')') => (true, &t[1..t.len() - 1]),
        None => (false, t),
    };
    // A currency's mark before or after the digits.
    let digits = rest.trim_matches(|c: char| c == '$' || c == ' ' || (!c.is_ascii() && !c.is_alphanumeric()));
    let starts = digits.bytes().next().is_some_and(|b| b.is_ascii_digit());
    if !starts || !digits.bytes().all(|b| b.is_ascii_digit() || b == b',' || b == b'.') {
        return None;
    }
    // Commas only as thousands separators before a decimal point (a decimal comma, or groups of
    // another size, are another locale's: not guessed).
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let mut groups = whole.split(',');
    let first = groups.next().unwrap_or("");
    let grouped = first.len() <= 3 && groups.all(|g| g.len() == 3);
    if frac.contains(',') || (whole.contains(',') && (!grouped || first.is_empty())) {
        return None;
    }
    let v: f64 = digits.replace(',', "").parse().ok()?;
    Some(if neg { -v } else { v })
}

/// Reads the rows of a result into a [`Model`] for a [`Spec`].
pub struct Builder<'a> {
    spec: &'a Spec,
    axis: Axis,
    texts: HashMap<String, usize>,
    numbers: HashMap<u64, usize>,
    points: Vec<Point>,
    series: Vec<Series>,
    names: HashMap<String, usize>,
    skipped: Skipped,
    rows: usize,
}

impl<'a> Builder<'a> {
    /// `columns` and `roles` of the result `spec` is for.
    pub fn new(spec: &'a Spec, columns: &[ColumnMeta], roles: &[Role]) -> Self {
        let axis = match spec.x.map(|x| roles.get(x).copied().unwrap_or(Role::Category)) {
            None => Axis::Row,
            Some(Role::Number) => Axis::Number,
            Some(Role::Time(k)) => Axis::Time(k),
            Some(_) => Axis::Category,
        };
        let series = match spec.by {
            Some(_) => Vec::new(),
            None => spec
                .ys
                .iter()
                .map(|&y| Series::new(columns.get(y).map(|c| c.name.clone()).unwrap_or_default(), false, 0))
                .collect(),
        };
        Self {
            spec,
            axis,
            texts: HashMap::new(),
            numbers: HashMap::new(),
            points: Vec::new(),
            series,
            names: HashMap::new(),
            skipped: Skipped::default(),
            rows: 0,
        }
    }

    /// Row `row` of the result (its cells).
    pub fn push(&mut self, row: usize, cells: &[Cell]) {
        self.rows += 1;
        // A row without a series makes no point.
        if self.spec.by.is_some_and(|by| cells.get(by).is_none_or(Option::is_none)) {
            self.skipped.null_by += 1;
            return;
        }
        let p = match self.spec.x {
            None => self.new_point((row + 1).to_string(), (row + 1) as f64, row),
            Some(x) => {
                let Some(text) = cells.get(x).and_then(Option::as_deref) else {
                    self.skipped.null_x += 1;
                    return;
                };
                let at = match self.axis {
                    Axis::Number => number(text),
                    Axis::Time(_) => time::parse(text).map(|p| p.secs),
                    _ => None,
                };
                match (self.axis, at) {
                    (Axis::Number | Axis::Time(_), None) => {
                        self.skipped.bad_x += 1;
                        return;
                    }
                    (_, Some(v)) => {
                        let key = if v == 0.0 { 0f64.to_bits() } else { v.to_bits() };
                        match self.numbers.get(&key) {
                            Some(&p) => p,
                            None => {
                                let p = self.new_point(text.trim().to_string(), v, row);
                                self.numbers.insert(key, p);
                                p
                            }
                        }
                    }
                    (_, None) => match self.texts.get(text) {
                        Some(&p) => p,
                        None => {
                            let p = self.new_point(text.to_string(), 0.0, row);
                            self.texts.insert(text.to_string(), p);
                            p
                        }
                    },
                }
            }
        };
        self.points[p].rows += 1;
        match self.spec.by {
            Some(by) => {
                // Not NULL: checked before the point was made.
                let Some(name) = cells.get(by).and_then(Option::as_deref) else { return };
                let s = match self.names.get(name) {
                    Some(&s) => s,
                    None => {
                        let s = self.series.len();
                        self.series.push(Series::new(name.to_string(), false, self.points.len()));
                        self.names.insert(name.to_string(), s);
                        s
                    }
                };
                if let Some(&y) = self.spec.ys.first() {
                    self.add(s, p, row, cells.get(y).and_then(Option::as_deref));
                }
            }
            None => {
                for s in 0..self.spec.ys.len() {
                    let y = self.spec.ys[s];
                    self.add(s, p, row, cells.get(y).and_then(Option::as_deref));
                }
            }
        }
    }

    fn new_point(&mut self, label: String, x: f64, row: usize) -> usize {
        self.points.push(Point { label, x, first_row: Some(row), rows: 0, others: false });
        for s in &mut self.series {
            s.values.push(None);
            s.rows.push(0);
            s.first.push(None);
            s.seen.push(false);
        }
        self.points.len() - 1
    }

    fn add(&mut self, s: usize, p: usize, row: usize, cell: Option<&str>) {
        self.series[s].seen[p] = true;
        let Some(text) = cell else {
            self.skipped.null_y += 1;
            return;
        };
        let Some(v) = number(text) else {
            self.skipped.bad_y += 1;
            return;
        };
        let s = &mut self.series[s];
        let sum = s.values[p].unwrap_or(0.0) + v;
        // A sum past the largest number is not drawn (counted as a value left out).
        if !sum.is_finite() {
            self.skipped.bad_y += 1;
            return;
        }
        s.values[p] = Some(sum);
        s.rows[p] += 1;
        s.first[p].get_or_insert(row);
    }

    pub fn finish(self) -> Result<Model, Unsuitable> {
        let Builder { spec, axis, mut points, mut series, mut skipped, rows, .. } = self;
        if rows == 0 {
            return Err(Unsuitable::NoRows);
        }
        if spec.ys.is_empty() {
            return Err(Unsuitable::NoSeries);
        }
        // Along a number or a time: in its order.
        if matches!(axis, Axis::Number | Axis::Time(_)) {
            let mut order: Vec<usize> = (0..points.len()).collect();
            order.sort_by(|&a, &b| points[a].x.total_cmp(&points[b].x));
            points = order.iter().map(|&i| points[i].clone()).collect();
            for s in &mut series {
                s.pick(&order);
            }
        }
        let mut other_series = 0;
        if series.len() > MAX_SERIES {
            let keep = largest(series.len(), MAX_SERIES - 1, |i| total(&series[i].values));
            let mut others = Series::new(String::new(), true, points.len());
            let mut kept = Vec::new();
            for (i, s) in series.into_iter().enumerate() {
                if keep.contains(&i) {
                    kept.push(s);
                } else {
                    other_series += 1;
                    for p in 0..points.len() {
                        if others.absorb(p, s.values[p], s.rows[p], s.seen[p]) {
                            skipped.bad_y += 1;
                        }
                    }
                }
            }
            kept.push(others);
            series = kept;
        }
        let mut other_points = 0;
        if spec.kind.bars() && points.len() > MAX_BARS {
            let weight = |i: usize| series.iter().filter_map(|s| s.values[i]).map(f64::abs).sum::<f64>();
            let keep = largest(points.len(), MAX_BARS - 1, weight);
            let mut sum = Point { label: String::new(), x: 0.0, first_row: None, rows: 0, others: true };
            let mut kept = Vec::new();
            for (i, p) in points.iter().enumerate() {
                if keep.contains(&i) {
                    kept.push(i);
                } else {
                    other_points += 1;
                    sum.rows += p.rows;
                }
            }
            for s in &mut series {
                // The others bar at the end: the first left-out point, with the rest summed in.
                let rest: Vec<usize> = (0..points.len()).filter(|i| !keep.contains(i)).collect();
                let order: Vec<usize> = kept.iter().copied().chain(rest.first().copied()).collect();
                let (values, rows, seen) = (s.values.clone(), s.rows.clone(), s.seen.clone());
                s.pick(&order);
                let last = order.len() - 1;
                s.first[last] = None;
                for &i in &rest[1..] {
                    if s.absorb(last, values[i], rows[i], seen[i]) {
                        skipped.bad_y += 1;
                    }
                }
            }
            points = kept.into_iter().map(|i| points[i].clone()).chain(std::iter::once(sum)).collect();
        }
        if axis == Axis::Category || spec.kind.bars() && axis != Axis::Row {
            for (i, p) in points.iter_mut().enumerate() {
                if axis == Axis::Category || p.others {
                    p.x = i as f64;
                }
            }
        }
        let values = series.iter().flat_map(|s| s.values.iter().flatten());
        if values.clone().next().is_none() {
            return Err(Unsuitable::NoValues);
        }
        let valued = |i: usize| series.iter().filter(|s| s.values[i].is_some()).count();
        let with_values = (0..points.len()).filter(|&i| valued(i) > 0).count();
        // One point is a comparison only between several values of it, as bars.
        let compared = with_values == 1 && spec.kind.bars() && (0..points.len()).any(|i| valued(i) >= 2);
        if with_values < 2 && !compared {
            return Err(Unsuitable::OnePoint);
        }
        let nonpositive = values.clone().filter(|v| **v <= 0.0).count();
        let widen = |r: Option<(f64, f64)>, v: f64| Some(r.map_or((v, v), |(a, b): (f64, f64)| (a.min(v), b.max(v))));
        let span = values.clone().fold(None, |r, v| widen(r, *v));
        let positive = values.filter(|v| **v > 0.0).fold(None, |r, v| widen(r, *v));
        let merged =
            series.iter().filter(|s| !s.others).any(|s| s.rows.iter().zip(&points).any(|(n, p)| *n > 1 && !p.others));
        Ok(Model {
            kind: spec.kind,
            axis,
            points,
            series,
            skipped,
            rows,
            merged,
            other_points,
            other_series,
            nonpositive,
            split: spec.by.is_some(),
            span,
            positive,
        })
    }
}

/// The sum of the sizes of `values`.
fn total(values: &[Option<f64>]) -> f64 {
    values.iter().flatten().map(|v| v.abs()).sum()
}

/// The indices of the `n` largest of `len` items by `weight` (ties: the earlier one).
fn largest(len: usize, n: usize, weight: impl Fn(usize) -> f64) -> std::collections::HashSet<usize> {
    let mut order: Vec<(usize, f64)> = (0..len).map(|i| (i, weight(i))).collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    order.into_iter().take(n).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests;
