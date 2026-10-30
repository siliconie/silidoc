//! Formulas in the maix store (MAIX-1.1 §4): a small, Excel-compatible calculator whose cell
//! references understand dimensions.
//!
//! * `=B2*1.18`, `=SUM(A1:A10)`, `=AVERAGE(...)`, `MIN`, `MAX`, `COUNT`, `ABS`, `ROUND`.
//! * `B2@{scenario=actual}`: the cell at the active dimension vector with `scenario` replaced.
//! * `DIMSUM(range, "scenario=actual")`, `DIMAVG`, `DIMCOUNT`: across **every** vector that
//!   contains the given pairs (an empty filter means all vectors).
//! * `DIMSLICE(B2, "scenario=budget")`: the same cell read at another vector (like the suffix).
//!
//! Formulas run inside the data store only. The sheet's own formula language belongs to GridCraft
//! and is not touched. A formula that cannot be worked out shows an Excel-style code as text
//! (`#DIV/0!`, `#NAME?`, `#PARSE!`, `#VALUE!`, `#REF!`, `#NUM!`).
//!
//! Cells read each other's **stored values**, and [`Maix::recalc`] repeats its pass until nothing
//! changes (at most [`MAX_PASSES`] times), so a chain settles in dependency order and a circular
//! reference simply stops after the last pass.

use crate::dims::DimVector;
use crate::maix::{Maix, Stamp, Value};

/// Most repeats of a recalculation pass.
pub const MAX_PASSES: usize = 8;
/// Most cells one range may cover.
const MAX_RANGE_CELLS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct At {
    row: u32,
    col: u16,
}

#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Num(f64),
    Text(String),
    Cell(At, Option<DimVector>),
    Range(At, At, Option<DimVector>),
    Neg(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

type Parsed<T> = Result<T, String>;

fn parse_error<T>() -> Parsed<T> {
    Err("#PARSE!".to_string())
}

/// `A1`, `$B$7`, `AA10` → zero-based position.
fn cell_ref(token: &str) -> Option<At> {
    let clean: String = token.chars().filter(|c| *c != '$').collect();
    let split = clean.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = clean.split_at(split);
    if letters.is_empty() || letters.len() > 3 || !letters.chars().all(|c| c.is_ascii_alphabetic()) || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let col = letters.chars().fold(0u32, |n, c| n * 26 + (u32::from(c.to_ascii_uppercase() as u8) - u32::from(b'A') + 1)) - 1;
    let row = digits.parse::<u32>().ok()?.checked_sub(1)?;
    Some(At { row, col: u16::try_from(col).ok()? })
}

/// `scenario=actual, role=estimator` (commas or semicolons) → a vector. Quotes around values are
/// dropped. An empty text is the empty vector.
pub fn parse_filter(text: &str) -> Option<DimVector> {
    let mut out = DimVector::new();
    for part in text.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=')?;
        let (k, v) = (k.trim(), v.trim().trim_matches('"').trim());
        if k.is_empty() {
            return None;
        }
        out.insert(k.to_string(), v.to_string());
    }
    Some(out)
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip_space();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Parsed<Expr> {
        let mut left = self.term()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(op @ ('+' | '-')) => {
                    self.i += 1;
                    let right = self.term()?;
                    left = Expr::Bin(op, Box::new(left), Box::new(right));
                }
                _ => return Ok(left),
            }
        }
    }

    fn term(&mut self) -> Parsed<Expr> {
        let mut left = self.power()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(op @ ('*' | '/')) => {
                    self.i += 1;
                    let right = self.power()?;
                    left = Expr::Bin(op, Box::new(left), Box::new(right));
                }
                _ => return Ok(left),
            }
        }
    }

    fn power(&mut self) -> Parsed<Expr> {
        let base = self.unary()?;
        if self.eat('^') {
            let exponent = self.unary()?;
            return Ok(Expr::Bin('^', Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    fn unary(&mut self) -> Parsed<Expr> {
        if self.eat('-') {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat('+') {
            return self.unary();
        }
        self.atom()
    }

    fn atom(&mut self) -> Parsed<Expr> {
        self.skip_space();
        match self.peek() {
            None => parse_error(),
            Some('(') => {
                self.i += 1;
                let inner = self.expr()?;
                if self.eat(')') { Ok(inner) } else { parse_error() }
            }
            Some('"') => self.string(),
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == '$' || c == '_' => self.name(),
            Some(_) => parse_error(),
        }
    }

    fn string(&mut self) -> Parsed<Expr> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return parse_error(),
                Some('"') => {
                    self.i += 1;
                    if self.peek() == Some('"') {
                        out.push('"'); // "" is a quote inside the text
                        self.i += 1;
                    } else {
                        return Ok(Expr::Text(out));
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Parsed<Expr> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            self.i += 1;
        }
        let text: String = self.s[start..self.i].iter().collect();
        text.parse::<f64>().map(Expr::Num).or_else(|_| parse_error())
    }

    /// A word made of letters, digits, `$` and `_`.
    fn word(&mut self) -> String {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '$' || c == '_') {
            self.i += 1;
        }
        self.s[start..self.i].iter().collect()
    }

    fn name(&mut self) -> Parsed<Expr> {
        let word = self.word();
        if let Some(first) = cell_ref(&word) {
            let mut last = first;
            let mut range = false;
            if self.peek() == Some(':') {
                self.i += 1;
                let second = self.word();
                last = cell_ref(&second).ok_or_else(|| "#REF!".to_string())?;
                range = true;
            }
            let over = self.suffix()?;
            return Ok(if range { Expr::Range(first, last, over) } else { Expr::Cell(first, over) });
        }
        if self.eat('(') {
            let mut args = vec![];
            if !self.eat(')') {
                loop {
                    args.push(self.expr()?);
                    if self.eat(',') || self.eat(';') {
                        continue;
                    }
                    if self.eat(')') {
                        break;
                    }
                    return parse_error();
                }
            }
            return Ok(Expr::Call(word.to_ascii_uppercase(), args));
        }
        Err("#NAME?".to_string())
    }

    /// `@{scenario=actual}` right after a reference.
    fn suffix(&mut self) -> Parsed<Option<DimVector>> {
        if self.peek() != Some('@') {
            return Ok(None);
        }
        self.i += 1;
        if self.peek() != Some('{') {
            return parse_error();
        }
        self.i += 1;
        let start = self.i;
        while self.peek().is_some_and(|c| c != '}') {
            self.i += 1;
        }
        if self.peek() != Some('}') {
            return parse_error();
        }
        let text: String = self.s[start..self.i].iter().collect();
        self.i += 1;
        match parse_filter(&text) {
            Some(v) if !v.is_empty() => Ok(Some(v)),
            _ => parse_error(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Out {
    Num(f64),
    Text(String),
    Empty,
}

impl Out {
    #[allow(clippy::cast_possible_truncation)]
    fn into_value(self) -> Value {
        match self {
            Out::Num(n) if n.is_finite() => {
                if n.fract() == 0.0 && n.abs() < 9e15 {
                    Value::Int(n as i64)
                } else {
                    Value::Float(n)
                }
            }
            Out::Num(_) => Value::Text("#NUM!".to_string()),
            Out::Text(t) => Value::Text(t),
            Out::Empty => Value::Int(0),
        }
    }
}

struct Ctx<'a> {
    maix: &'a Maix,
    sheet: u16,
    dims: &'a DimVector,
}

fn merged(base: &DimVector, over: Option<&DimVector>) -> DimVector {
    let mut out = base.clone();
    if let Some(o) = over {
        out.extend(o.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    out
}

fn out_of(v: &Value) -> Out {
    match v {
        Value::Null => Out::Empty,
        Value::Int(i) => Out::Num(*i as f64),
        Value::Float(f) => Out::Num(*f),
        Value::Bool(b) => Out::Num(if *b { 1.0 } else { 0.0 }),
        Value::Date(ms) => Out::Num(*ms as f64),
        Value::Text(t) => Out::Text(t.clone()),
    }
}

fn read(cx: &Ctx, at: At, dims: &DimVector) -> Out {
    match cx.maix.get(cx.sheet, at.row, at.col, dims) {
        Ok(Some(v)) => out_of(v),
        _ => Out::Empty,
    }
}

fn number(out: &Out) -> Result<f64, String> {
    match out {
        Out::Num(n) => Ok(*n),
        Out::Empty => Ok(0.0),
        Out::Text(t) => t.trim().parse::<f64>().map_err(|_| "#VALUE!".to_string()),
    }
}

/// The rectangle an argument names: a range, or a single cell.
fn rect(arg: &Expr) -> Result<(At, At, Option<&DimVector>), String> {
    match arg {
        Expr::Range(a, b, o) => Ok((At { row: a.row.min(b.row), col: a.col.min(b.col) }, At { row: a.row.max(b.row), col: a.col.max(b.col) }, o.as_ref())),
        Expr::Cell(a, o) => Ok((*a, *a, o.as_ref())),
        _ => Err("#VALUE!".to_string()),
    }
}

fn each_cell(a: At, b: At) -> Result<impl Iterator<Item = At>, String> {
    let count = (u64::from(b.row - a.row) + 1) * (u64::from(b.col - a.col) + 1);
    if count > MAX_RANGE_CELLS {
        return Err("#REF!".to_string());
    }
    Ok((a.row..=b.row).flat_map(move |row| (a.col..=b.col).map(move |col| At { row, col })))
}

/// The numbers an argument stands for at the current vector (ranges and single values).
fn numbers(arg: &Expr, cx: &Ctx) -> Result<Vec<f64>, String> {
    match arg {
        Expr::Range(..) | Expr::Cell(..) => {
            let (a, b, over) = rect(arg)?;
            let dims = merged(cx.dims, over);
            Ok(each_cell(a, b)?.filter_map(|at| if let Out::Num(n) = read(cx, at, &dims) { Some(n) } else { None }).collect())
        }
        other => Ok(vec![number(&eval(other, cx)?)?]),
    }
}

fn filter_of(arg: Option<&Expr>, cx: &Ctx) -> Result<DimVector, String> {
    match arg {
        None => Ok(DimVector::new()),
        Some(e) => match eval(e, cx)? {
            Out::Text(t) => parse_filter(&t).ok_or_else(|| "#VALUE!".to_string()),
            Out::Empty => Ok(DimVector::new()),
            Out::Num(_) => Err("#VALUE!".to_string()),
        },
    }
}

/// The numbers in a range under every vector that contains all pairs of `filter`.
fn across(arg: &Expr, filter: &DimVector, cx: &Ctx) -> Result<Vec<f64>, String> {
    let (a, b, _) = rect(arg)?;
    let sheet = cx.maix.sheet(cx.sheet).map_err(|_| "#REF!".to_string())?;
    let mut out = vec![];
    for at in each_cell(a, b)? {
        for (dim, cell) in sheet.across(at.row, at.col) {
            let hit = filter.is_empty() || cx.maix.vectors.resolve(dim).is_some_and(|v| filter.iter().all(|(k, x)| v.get(k) == Some(x)));
            if !hit {
                continue;
            }
            match &cell.value {
                Value::Int(i) => out.push(*i as f64),
                Value::Float(f) => out.push(*f),
                _ => {}
            }
        }
    }
    Ok(out)
}

fn call(name: &str, args: &[Expr], cx: &Ctx) -> Result<Out, String> {
    let all_numbers = |args: &[Expr]| -> Result<Vec<f64>, String> {
        let mut v = vec![];
        for a in args {
            v.extend(numbers(a, cx)?);
        }
        Ok(v)
    };
    let first = args.first().ok_or_else(|| "#VALUE!".to_string());
    match name {
        "SUM" => Ok(Out::Num(all_numbers(args)?.iter().sum())),
        "COUNT" => Ok(Out::Num(all_numbers(args)?.len() as f64)),
        "AVERAGE" => {
            let v = all_numbers(args)?;
            if v.is_empty() { Err("#DIV/0!".to_string()) } else { Ok(Out::Num(v.iter().sum::<f64>() / v.len() as f64)) }
        }
        "MIN" => Ok(Out::Num(all_numbers(args)?.into_iter().reduce(f64::min).unwrap_or(0.0))),
        "MAX" => Ok(Out::Num(all_numbers(args)?.into_iter().reduce(f64::max).unwrap_or(0.0))),
        "ABS" => Ok(Out::Num(number(&eval(first?, cx)?)?.abs())),
        "ROUND" => {
            let x = number(&eval(first?, cx)?)?;
            let digits = match args.get(1) {
                Some(e) => number(&eval(e, cx)?)?,
                None => 0.0,
            };
            let scale = 10f64.powf(digits.trunc());
            Ok(Out::Num((x * scale).round() / scale))
        }
        "DIMSUM" | "DIMAVG" | "DIMCOUNT" => {
            let filter = filter_of(args.get(1), cx)?;
            let v = across(first?, &filter, cx)?;
            match name {
                "DIMSUM" => Ok(Out::Num(v.iter().sum())),
                "DIMCOUNT" => Ok(Out::Num(v.len() as f64)),
                _ if v.is_empty() => Err("#DIV/0!".to_string()),
                _ => Ok(Out::Num(v.iter().sum::<f64>() / v.len() as f64)),
            }
        }
        "DIMSLICE" => {
            let Expr::Cell(at, over) = first? else {
                return Err("#VALUE!".to_string());
            };
            let filter = filter_of(args.get(1), cx)?;
            let dims = merged(&merged(cx.dims, over.as_ref()), Some(&filter));
            Ok(read(cx, *at, &dims))
        }
        _ => Err("#NAME?".to_string()),
    }
}

fn eval(e: &Expr, cx: &Ctx) -> Result<Out, String> {
    match e {
        Expr::Num(n) => Ok(Out::Num(*n)),
        Expr::Text(t) => Ok(Out::Text(t.clone())),
        Expr::Cell(at, over) => Ok(read(cx, *at, &merged(cx.dims, over.as_ref()))),
        Expr::Range(..) => Err("#VALUE!".to_string()),
        Expr::Neg(x) => Ok(Out::Num(-number(&eval(x, cx)?)?)),
        Expr::Bin(op, l, r) => {
            let (a, b) = (number(&eval(l, cx)?)?, number(&eval(r, cx)?)?);
            match op {
                '+' => Ok(Out::Num(a + b)),
                '-' => Ok(Out::Num(a - b)),
                '*' => Ok(Out::Num(a * b)),
                '/' if b == 0.0 => Err("#DIV/0!".to_string()),
                '/' => Ok(Out::Num(a / b)),
                _ => Ok(Out::Num(a.powf(b))),
            }
        }
        Expr::Call(name, args) => call(name, args, cx),
    }
}

/// Works out `formula` for a cell of `sheet` at `dims`. Never fails: a problem is the value.
pub fn evaluate(maix: &Maix, sheet: u16, dims: &DimVector, formula: &str) -> Value {
    let text = formula.trim();
    let chars: Vec<char> = text.strip_prefix('=').unwrap_or(text).chars().collect();
    let mut parser = Parser { s: &chars, i: 0 };
    let parsed = parser.expr();
    parser.skip_space();
    let result = match parsed {
        Ok(expr) if parser.i >= chars.len() => eval(&expr, &Ctx { maix, sheet, dims }),
        Ok(_) => parse_error(),
        Err(code) => Err(code),
    };
    match result {
        Ok(out) => out.into_value(),
        Err(code) => Value::Text(code),
    }
}

impl Maix {
    /// Works out every formula in the store and writes the results as values. Returns how many
    /// values changed.
    pub fn recalc(&mut self, by: &Stamp) -> usize {
        let mut total = 0;
        for _ in 0..MAX_PASSES {
            let mut jobs: Vec<(u16, u32, u16, DimVector, String)> = vec![];
            for sheet in &self.sheets {
                for (row, col, dim, formula) in sheet.formulas() {
                    jobs.push((sheet.id, row, col, self.vectors.resolve(dim).cloned().unwrap_or_default(), formula));
                }
            }
            if jobs.is_empty() {
                break;
            }
            let mut changed = 0;
            for (sheet, row, col, dims, formula) in jobs {
                let value = evaluate(self, sheet, &dims, &formula);
                if self.set(sheet, row, col, &dims, value, by).unwrap_or(false) {
                    changed += 1;
                }
            }
            total += changed;
            if changed == 0 {
                break;
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims(pairs: &[(&str, &str)]) -> DimVector {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    fn put(m: &mut Maix, sheet: u16, row: u32, col: u16, d: &DimVector, v: Value) {
        m.set(sheet, row, col, d, v, &Stamp::new("t", 1)).unwrap();
    }

    fn calc(m: &Maix, s: u16, f: &str) -> Value {
        evaluate(m, s, &DimVector::new(), f)
    }

    /// A1:A3 = 10, 20, 30 with no dimensions.
    fn store() -> (Maix, u16) {
        let mut m = Maix::default();
        let s = m.add_sheet("Costs");
        for (i, n) in [10, 20, 30].into_iter().enumerate() {
            put(&mut m, s, i as u32, 0, &DimVector::new(), Value::Int(n));
        }
        (m, s)
    }

    #[test]
    fn arithmetic_and_functions() {
        let (m, s) = store();
        assert_eq!(calc(&m, s, "=A1+A2*2"), Value::Int(50));
        assert_eq!(calc(&m, s, "=(A1+A2)*2"), Value::Int(60));
        assert_eq!(calc(&m, s, "=SUM(A1:A3)"), Value::Int(60));
        assert_eq!(calc(&m, s, "=AVERAGE(A1:A3)"), Value::Int(20));
        assert_eq!(calc(&m, s, "=MAX(A1:A3)-MIN(A1:A3)"), Value::Int(20));
        assert_eq!(calc(&m, s, "=COUNT(A1:A5)"), Value::Int(3));
        assert_eq!(calc(&m, s, "=ROUND(10/3,2)"), Value::Float(3.33));
        assert_eq!(calc(&m, s, "=-A1^2"), Value::Int(100)); // unary minus binds tighter, as in Excel
        assert_eq!(calc(&m, s, "=sum(a1:a2)"), Value::Int(30));
    }

    #[test]
    fn mistakes_show_excel_style_codes() {
        let (m, s) = store();
        assert_eq!(calc(&m, s, "=A1/0"), Value::Text("#DIV/0!".into()));
        assert_eq!(calc(&m, s, "=NOPE(1)"), Value::Text("#NAME?".into()));
        assert_eq!(calc(&m, s, "=1+"), Value::Text("#PARSE!".into()));
        assert_eq!(calc(&m, s, "=1 2"), Value::Text("#PARSE!".into()));
        assert_eq!(calc(&m, s, "=\"x\"+1"), Value::Text("#VALUE!".into()));
        assert_eq!(calc(&m, s, "=A1@{oops}"), Value::Text("#PARSE!".into()));
    }

    #[test]
    fn the_suffix_reads_another_vector() {
        let (mut m, s) = store();
        put(&mut m, s, 0, 0, &dims(&[("scenario", "budget")]), Value::Int(100));
        assert_eq!(calc(&m, s, "=A1@{scenario=budget}"), Value::Int(100));
        assert_eq!(calc(&m, s, "=A1@{scenario=budget}-A1"), Value::Int(90));
        // At an active vector the suffix replaces only what it names.
        let active = dims(&[("scenario", "budget"), ("role", "estimator")]);
        put(&mut m, s, 0, 0, &active, Value::Int(7));
        assert_eq!(evaluate(&m, s, &active, "=A1"), Value::Int(7));
        assert_eq!(evaluate(&m, s, &active, "=A1@{scenario=actual}"), Value::Int(0)); // nothing there
    }

    #[test]
    fn dimsum_adds_across_the_matching_vectors() {
        let (mut m, s) = store();
        let q1 = dims(&[("time", "2026-Q1"), ("scenario", "actual")]);
        let q2 = dims(&[("time", "2026-Q2"), ("scenario", "actual")]);
        let plan = dims(&[("time", "2026-Q1"), ("scenario", "budget")]);
        put(&mut m, s, 0, 1, &q1, Value::Int(5));
        put(&mut m, s, 0, 1, &q2, Value::Int(6));
        put(&mut m, s, 0, 1, &plan, Value::Int(100));
        assert_eq!(calc(&m, s, "=DIMSUM(B1,\"scenario=actual\")"), Value::Int(11));
        assert_eq!(calc(&m, s, "=DIMSUM(B1:B1,\"scenario=budget\")"), Value::Int(100));
        assert_eq!(calc(&m, s, "=DIMSUM(B1,\"time=2026-Q1\")"), Value::Int(105));
        assert_eq!(calc(&m, s, "=DIMSUM(B1)"), Value::Int(111));
        assert_eq!(calc(&m, s, "=DIMCOUNT(B1,\"scenario=actual\")"), Value::Int(2));
        assert_eq!(calc(&m, s, "=DIMAVG(B1,\"scenario=actual\")"), Value::Float(5.5));
        assert_eq!(calc(&m, s, "=DIMAVG(B1,\"scenario=nothing\")"), Value::Text("#DIV/0!".into()));
    }

    #[test]
    fn dimslice_reads_the_same_cell_at_another_vector() {
        let (mut m, s) = store();
        put(&mut m, s, 1, 0, &dims(&[("scenario", "budget")]), Value::Int(25));
        assert_eq!(calc(&m, s, "=DIMSLICE(A2,\"scenario=budget\")"), Value::Int(25));
        assert_eq!(calc(&m, s, "=DIMSLICE(A2,\"scenario=budget\")-A2"), Value::Int(5));
    }

    #[test]
    fn recalc_settles_chains_and_stops_on_cycles() {
        let (mut m, s) = store();
        let none = DimVector::new();
        // C1 depends on B1, which depends on A1: written "backwards" on purpose.
        m.set_formula(s, 0, 2, &none, Some("=B1+1")).unwrap();
        m.set_formula(s, 0, 1, &none, Some("=A1*2")).unwrap();
        m.recalc(&Stamp::new("t", 2));
        assert_eq!(m.get(s, 0, 1, &none).unwrap(), Some(&Value::Int(20)));
        assert_eq!(m.get(s, 0, 2, &none).unwrap(), Some(&Value::Int(21)));
        // An edit flows through on the next recalculation; nothing changes the time after.
        put(&mut m, s, 0, 0, &none, Value::Int(1));
        assert!(m.recalc(&Stamp::new("t", 3)) > 0);
        assert_eq!(m.get(s, 0, 2, &none).unwrap(), Some(&Value::Int(3)));
        assert_eq!(m.recalc(&Stamp::new("t", 4)), 0);
        // A circular reference ends (it does not hang).
        m.set_formula(s, 5, 0, &none, Some("=A7+1")).unwrap();
        m.set_formula(s, 6, 0, &none, Some("=A6+1")).unwrap();
        assert!(m.recalc(&Stamp::new("t", 5)) <= MAX_PASSES * 2);
    }

    #[test]
    fn formulas_can_be_set_read_and_removed() {
        let (mut m, s) = store();
        let none = DimVector::new();
        assert!(m.set_formula(s, 0, 3, &none, Some("=A1")).unwrap());
        assert!(!m.set_formula(s, 0, 3, &none, Some("=A1")).unwrap());
        assert_eq!(m.formula(s, 0, 3, &none), Some("=A1"));
        assert!(m.set_formula(s, 0, 3, &none, None).unwrap());
        assert_eq!(m.formula(s, 0, 3, &none), None);
        assert!(!m.set_formula(s, 9, 9, &dims(&[("x", "y")]), None).unwrap());
    }

    #[test]
    fn filters_parse_loosely() {
        assert_eq!(parse_filter("scenario=actual; role = \"est\""), Some(dims(&[("scenario", "actual"), ("role", "est")])));
        assert_eq!(parse_filter("  "), Some(DimVector::new()));
        assert_eq!(parse_filter("nonsense"), None);
    }
}
