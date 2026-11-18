//! Bank statements: one generic CSV reader, configured per bank by a
//! mapping (ADR-0020 decision 3, `docs/reconciliation/statement-format.md`).
//!
//! A mapping names the bank's columns, its date format, its decimal and
//! thousands separators, and how a movement's direction is written: one
//! signed amount column, or separate debit and credit columns. Every line
//! comes out the same way: the bank's line ID, the booking date, a signed
//! amount in minor units (positive is money into the account), the currency
//! and the payment reference. Nothing is rounded or guessed: a line that
//! does not read exactly is an error naming its line number.

use std::collections::BTreeMap;
use std::fmt;

use mezgeb_core::{Currency, Money};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// How the statement's dates are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum DateFormat {
    #[serde(rename = "YYYY-MM-DD")]
    IsoDash,
    #[serde(rename = "YYYYMMDD")]
    Compact,
    #[serde(rename = "DD/MM/YYYY")]
    DaySlash,
    #[serde(rename = "MM/DD/YYYY")]
    MonthSlash,
    #[serde(rename = "DD.MM.YYYY")]
    DayDot,
    #[serde(rename = "DD-MM-YYYY")]
    DayDash,
}

/// How a signed amount column shows the direction of a movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    /// Money in (a credit on the bank's statement) is positive.
    #[default]
    CreditPositive,
    /// Money out (a debit on the bank's statement) is positive.
    DebitPositive,
}

/// The bank's column names, as in its header row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Columns {
    /// The bank's own ID for the line, unique within the account.
    pub line_id: String,
    /// The booking date.
    pub date: String,
    /// One signed amount column; or `debit` and `credit` instead.
    pub amount: Option<String>,
    pub debit: Option<String>,
    pub credit: Option<String>,
    /// The payment reference, such as a customer's code.
    pub reference: Option<String>,
    /// The currency, if the statement has a column for it.
    pub currency: Option<String>,
}

/// How to read one bank's statement export.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    /// A name for the bank's format, recorded with each import.
    pub name: String,
    #[serde(default = "comma")]
    pub delimiter: char,
    /// Lines before the header row, such as a title or account details.
    #[serde(default)]
    pub skip_lines: usize,
    pub date_format: DateFormat,
    #[serde(default = "dot")]
    pub decimal_separator: char,
    /// Grouping separator inside amounts, if the bank prints one.
    #[serde(default)]
    pub thousands_separator: Option<char>,
    /// The currency of every line, when there is no currency column.
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub sign: Sign,
    pub columns: Columns,
}

fn comma() -> char {
    ','
}

fn dot() -> char {
    '.'
}

impl Mapping {
    /// Mezgeb's own layout (`docs/reconciliation/statement-format.md`):
    /// `line_id,booking_date,amount,currency,reference`, ISO dates, `.`
    /// decimals and money in positive.
    pub fn mezgeb() -> Mapping {
        Mapping {
            name: "mezgeb-csv-v1".to_owned(),
            delimiter: ',',
            skip_lines: 0,
            date_format: DateFormat::IsoDash,
            decimal_separator: '.',
            thousands_separator: None,
            currency: None,
            sign: Sign::CreditPositive,
            columns: Columns {
                line_id: "line_id".to_owned(),
                date: "booking_date".to_owned(),
                amount: Some("amount".to_owned()),
                debit: None,
                credit: None,
                reference: Some("reference".to_owned()),
                currency: Some("currency".to_owned()),
            },
        }
    }

    /// Reads a mapping file and checks that it can describe a statement.
    pub fn from_toml(text: &str) -> Result<Mapping, StatementError> {
        let mapping: Mapping =
            toml::from_str(text).map_err(|e| StatementError::mapping(e.message().to_owned()))?;
        mapping.check()?;
        Ok(mapping)
    }

    fn check(&self) -> Result<(), StatementError> {
        if self.name.trim().is_empty() || self.name.len() > 100 {
            return Err(StatementError::mapping("name must be 1 to 100 characters"));
        }
        let c = &self.columns;
        match (&c.amount, &c.debit, &c.credit) {
            (Some(_), None, None) | (None, Some(_), Some(_)) => {}
            _ => {
                return Err(StatementError::mapping(
                    "give either `amount`, or both `debit` and `credit`",
                ));
            }
        }
        if self.decimal_separator == self.delimiter
            || Some(self.decimal_separator) == self.thousands_separator
        {
            return Err(StatementError::mapping(
                "the decimal separator must differ from the delimiter and the thousands separator",
            ));
        }
        if let Some(code) = &self.currency {
            Currency::from_code(code)
                .map_err(|_| StatementError::mapping(format!("unknown currency {code}")))?;
        }
        Ok(())
    }
}

/// One statement line, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementLine {
    /// The line number in the file, from 1, for messages.
    pub row: usize,
    pub line_ref: String,
    /// Days since 1970-01-01.
    pub booking_date: i32,
    /// Signed, in minor units: positive is money into the account.
    pub amount_minor: i64,
    pub currency: Currency,
    pub reference: String,
}

impl StatementLine {
    /// SHA-256 of the line's content: the same line ID imported again with
    /// another digest is a statement conflict (ADR-0020 decision 4).
    pub fn digest(&self) -> [u8; 32] {
        let text = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.line_ref,
            format_date(self.booking_date),
            self.amount_minor,
            self.currency.code(),
            self.reference
        );
        Sha256::digest(text.as_bytes()).into()
    }
}

/// Why a statement or mapping could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementError {
    /// The file line it is about, from 1; `None` for the mapping or the
    /// file as a whole.
    pub row: Option<usize>,
    pub message: String,
}

impl StatementError {
    fn mapping(message: impl Into<String>) -> StatementError {
        StatementError {
            row: None,
            message: format!("mapping: {}", message.into()),
        }
    }

    fn at(row: usize, message: impl Into<String>) -> StatementError {
        StatementError {
            row: Some(row),
            message: message.into(),
        }
    }
}

impl fmt::Display for StatementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.row {
            Some(row) => write!(f, "line {row}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for StatementError {}

/// Reads a statement for an account in `currency`. Every line must be in
/// that currency.
pub fn parse_statement(
    text: &str,
    mapping: &Mapping,
    currency: Currency,
) -> Result<Vec<StatementLine>, StatementError> {
    mapping.check()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let records = records(text, mapping.delimiter)?;
    let mut records = records
        .into_iter()
        .skip_while(|(row, _)| *row <= mapping.skip_lines);
    let (header_row, header) = records
        .next()
        .ok_or_else(|| StatementError::at(mapping.skip_lines + 1, "no header row"))?;
    let index: BTreeMap<String, usize> = header
        .iter()
        .enumerate()
        .map(|(i, name)| (name.trim().to_owned(), i))
        .collect();
    let column = |name: &Option<String>| -> Result<Option<usize>, StatementError> {
        name.as_ref()
            .map(|n| {
                index
                    .get(n.trim())
                    .copied()
                    .ok_or_else(|| StatementError::at(header_row, format!("no column named {n:?}")))
            })
            .transpose()
    };
    let c = &mapping.columns;
    let line_id = column(&Some(c.line_id.clone()))?.unwrap_or_default();
    let date = column(&Some(c.date.clone()))?.unwrap_or_default();
    let amount = column(&c.amount)?;
    let debit = column(&c.debit)?;
    let credit = column(&c.credit)?;
    let reference = column(&c.reference)?;
    let currency_column = column(&c.currency)?;
    let fixed_currency = mapping.currency.as_deref().unwrap_or(currency.code());

    let mut lines = Vec::new();
    for (row, fields) in records {
        if fields.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        let field = |i: usize| fields.get(i).map_or("", |f| f.trim());
        let line_ref = field(line_id);
        if line_ref.is_empty() || line_ref.chars().count() > 100 {
            return Err(StatementError::at(
                row,
                "the line ID must be 1 to 100 characters",
            ));
        }
        let booking_date = parse_date(field(date), mapping.date_format)
            .ok_or_else(|| StatementError::at(row, format!("bad date {:?}", field(date))))?;
        let code = currency_column.map_or(fixed_currency, field);
        if code != currency.code() {
            return Err(StatementError::at(
                row,
                format!("currency {code} is not the account's {}", currency.code()),
            ));
        }
        let amount_minor = match (amount, debit, credit) {
            (Some(i), _, _) => {
                let value = parse_amount(field(i), mapping, currency, row)?;
                match mapping.sign {
                    Sign::CreditPositive => value,
                    Sign::DebitPositive => value
                        .checked_neg()
                        .ok_or_else(|| StatementError::at(row, "amount out of range"))?,
                }
            }
            (None, Some(d), Some(c)) => match (field(d), field(c)) {
                (d, "") if !d.is_empty() => parse_amount(d, mapping, currency, row)?
                    .checked_neg()
                    .ok_or_else(|| StatementError::at(row, "amount out of range"))?,
                ("", c) if !c.is_empty() => parse_amount(c, mapping, currency, row)?,
                _ => {
                    return Err(StatementError::at(
                        row,
                        "exactly one of the debit and credit columns must have an amount",
                    ));
                }
            },
            _ => return Err(StatementError::mapping("no amount columns")),
        };
        if amount_minor == 0 {
            return Err(StatementError::at(row, "the amount is zero"));
        }
        let reference = reference.map_or("", field);
        if reference.chars().count() > 200 {
            return Err(StatementError::at(
                row,
                "the reference is over 200 characters",
            ));
        }
        lines.push(StatementLine {
            row,
            line_ref: line_ref.to_owned(),
            booking_date,
            amount_minor,
            currency,
            reference: reference.to_owned(),
        });
    }
    Ok(lines)
}

/// An amount as the bank writes it, in minor units. A leading `-` or
/// surrounding parentheses make it negative; a leading `+` is allowed.
fn parse_amount(
    text: &str,
    mapping: &Mapping,
    currency: Currency,
    row: usize,
) -> Result<i64, StatementError> {
    let bad = || StatementError::at(row, format!("bad amount {text:?}"));
    let (negative, body) =
        if let Some(inner) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
            (true, inner.trim())
        } else if let Some(rest) = text.strip_prefix('-') {
            (true, rest.trim())
        } else {
            (false, text.strip_prefix('+').unwrap_or(text).trim())
        };
    let mut plain = String::with_capacity(body.len());
    for ch in body.chars() {
        if Some(ch) == mapping.thousands_separator {
            continue;
        }
        if ch == mapping.decimal_separator {
            plain.push('.');
        } else if ch.is_ascii_digit() {
            plain.push(ch);
        } else {
            return Err(bad());
        }
    }
    // Banks pad with zeros ("0015.00"); the exact parser does not.
    let trimmed = plain.trim_start_matches('0');
    let plain = if trimmed.is_empty() || trimmed.starts_with('.') {
        format!("0{trimmed}")
    } else {
        trimmed.to_owned()
    };
    let money = Money::parse(&plain, currency).map_err(|_| bad())?;
    let value = money.amount();
    if negative { Ok(-value) } else { Ok(value) }
}

/// Splits CSV text into records (RFC 4180: quoted fields may hold the
/// delimiter, `""` and line breaks), each with the file line it starts on.
fn records(text: &str, delimiter: char) -> Result<Vec<(usize, Vec<String>)>, StatementError> {
    let mut records = Vec::new();
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut row = 1;
    let mut start = 1;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quotes {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                '\n' => {
                    row += 1;
                    field.push('\n');
                }
                _ => field.push(ch),
            }
            continue;
        }
        match ch {
            '"' if field.trim().is_empty() => {
                field.clear();
                in_quotes = true;
            }
            c if c == delimiter => fields.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                fields.push(std::mem::take(&mut field));
                records.push((start, std::mem::take(&mut fields)));
                row += 1;
                start = row;
            }
            _ => field.push(ch),
        }
    }
    if in_quotes {
        return Err(StatementError::at(start, "a quoted field is not closed"));
    }
    if !field.is_empty() || !fields.is_empty() {
        fields.push(field);
        records.push((start, fields));
    }
    Ok(records)
}

/// A date in the given format, as days since 1970-01-01.
pub fn parse_date(text: &str, format: DateFormat) -> Option<i32> {
    let digits = |s: &str| -> Option<u32> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let split = |sep: char| -> Option<(u32, u32, u32)> {
        let mut parts = text.split(sep);
        let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        Some((digits(a)?, digits(b)?, digits(c)?))
    };
    let (year, month, day) = match format {
        DateFormat::IsoDash => {
            let (y, m, d) = split('-')?;
            (y, m, d)
        }
        DateFormat::Compact => {
            if text.len() != 8 {
                return None;
            }
            (
                digits(text.get(..4)?)?,
                digits(text.get(4..6)?)?,
                digits(text.get(6..)?)?,
            )
        }
        DateFormat::DaySlash => {
            let (d, m, y) = split('/')?;
            (y, m, d)
        }
        DateFormat::MonthSlash => {
            let (m, d, y) = split('/')?;
            (y, m, d)
        }
        DateFormat::DayDot => {
            let (d, m, y) = split('.')?;
            (y, m, d)
        }
        DateFormat::DayDash => {
            let (d, m, y) = split('-')?;
            (y, m, d)
        }
    };
    days_from_civil(year, month, day)
}

/// Days since 1970-01-01 for a Gregorian date, if it exists (years 1900 to
/// 9999).
pub fn days_from_civil(year: u32, month: u32, day: u32) -> Option<i32> {
    if !(1900..=9999).contains(&year) || !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let length = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day > length {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i32::try_from(era * 146_097 + doe - 719_468).ok()
}

/// A day number as `YYYY-MM-DD`.
pub fn format_date(days: i32) -> String {
    let z = i64::from(days) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ETB: Currency = Currency::ETB;

    #[test]
    fn mezgeb_layout_reads() {
        let text = "line_id,booking_date,amount,currency,reference\n\
                    L1,2026-11-16,1000.00,ETB,HANA\n\
                    L2,2026-11-16,-15,ETB,\"monthly charge, Nov\"\n";
        let lines = parse_statement(text, &Mapping::mezgeb(), ETB).expect("reads");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line_ref, "L1");
        assert_eq!(lines[0].amount_minor, 100_000);
        assert_eq!(format_date(lines[0].booking_date), "2026-11-16");
        assert_eq!(lines[1].amount_minor, -1_500);
        assert_eq!(lines[1].reference, "monthly charge, Nov");
        assert_eq!(lines[1].row, 3);
    }

    #[test]
    fn a_bank_mapping_reads_its_own_layout() {
        let mapping = Mapping::from_toml(
            r#"
            name = "Example Bank export"
            delimiter = ";"
            skip_lines = 2
            date_format = "DD/MM/YYYY"
            decimal_separator = ","
            thousands_separator = "."
            currency = "ETB"

            [columns]
            line_id = "Txn ID"
            date = "Value date"
            debit = "Debit"
            credit = "Credit"
            reference = "Narrative"
            "#,
        )
        .expect("mapping reads");
        let text = "Example Bank statement\r\nAccount 1000123\r\n\
                    Txn ID;Value date;Narrative;Debit;Credit\r\n\
                    T-1;16/11/2026;HANA;;1.000,00\r\n\
                    T-2;17/11/2026;Charge;15,00;\r\n\
                    \r\n";
        let lines = parse_statement(text, &mapping, ETB).expect("reads");
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.line_ref.as_str(), l.amount_minor))
                .collect::<Vec<_>>(),
            [("T-1", 100_000), ("T-2", -1_500)]
        );
        assert_eq!(format_date(lines[1].booking_date), "2026-11-17");
        assert_eq!(lines[0].row, 4);
    }

    #[test]
    fn debit_positive_banks_flip_the_sign() {
        let mut mapping = Mapping::mezgeb();
        mapping.sign = Sign::DebitPositive;
        let text = "line_id,booking_date,amount,currency,reference\nL1,2026-11-16,15.00,ETB,\n";
        let lines = parse_statement(text, &mapping, ETB).expect("reads");
        assert_eq!(lines[0].amount_minor, -1_500);
    }

    #[test]
    fn what_does_not_read_exactly_is_refused_with_its_line() {
        let header = "line_id,booking_date,amount,currency,reference\n";
        for (body, message) in [
            ("L1,2026-02-30,1.00,ETB,\n", "bad date"),
            ("L1,2026-11-16,1.001,ETB,\n", "bad amount"),
            ("L1,2026-11-16,1.00,USD,\n", "currency USD"),
            ("L1,2026-11-16,0.00,ETB,\n", "zero"),
            (",2026-11-16,1.00,ETB,\n", "line ID"),
            ("L1,2026-11-16,\"1.00,ETB,\n", "not closed"),
            ("L1,2026-11-16,1e3,ETB,\n", "bad amount"),
        ] {
            let error = parse_statement(&format!("{header}{body}"), &Mapping::mezgeb(), ETB)
                .expect_err(body);
            assert_eq!(error.row, Some(2), "{body}");
            assert!(error.message.contains(message), "{body}: {error}");
        }
        let error = parse_statement("id,date\n", &Mapping::mezgeb(), ETB).expect_err("header");
        assert!(error.message.contains("no column named \"line_id\""));
    }

    #[test]
    fn a_mapping_must_describe_the_amount_once() {
        let error = Mapping::from_toml(
            r#"
            name = "x"
            date_format = "YYYY-MM-DD"
            [columns]
            line_id = "id"
            date = "d"
            amount = "a"
            debit = "dr"
            "#,
        )
        .expect_err("two ways");
        assert!(error.message.contains("either `amount`"));
        assert!(Mapping::from_toml("name = \"x\"\ndate_format = \"DD MM\"\n").is_err());
    }

    #[test]
    fn dates_round_trip() {
        for (format, text, iso) in [
            (DateFormat::IsoDash, "2024-02-29", "2024-02-29"),
            (DateFormat::Compact, "19700101", "1970-01-01"),
            (DateFormat::DaySlash, "01/03/2026", "2026-03-01"),
            (DateFormat::MonthSlash, "03/01/2026", "2026-03-01"),
            (DateFormat::DayDot, "31.12.1999", "1999-12-31"),
            (DateFormat::DayDash, "28-02-2100", "2100-02-28"),
        ] {
            let days = parse_date(text, format).expect(text);
            assert_eq!(format_date(days), iso);
        }
        assert_eq!(parse_date("1970-01-01", DateFormat::IsoDash), Some(0));
        assert_eq!(parse_date("2100-02-29", DateFormat::IsoDash), None);
        assert_eq!(parse_date("2026-11-1x", DateFormat::IsoDash), None);
        for days in (-25_000..80_000).step_by(37) {
            let iso = format_date(days);
            assert_eq!(parse_date(&iso, DateFormat::IsoDash), Some(days), "{iso}");
        }
    }

    #[test]
    fn the_digest_changes_with_any_field() {
        let line = StatementLine {
            row: 1,
            line_ref: "L1".into(),
            booking_date: 20_000,
            amount_minor: 100,
            currency: ETB,
            reference: "A".into(),
        };
        let base = line.digest();
        let mut other = line.clone();
        other.row = 9;
        assert_eq!(other.digest(), base, "the file line is not content");
        for change in [
            |l: &mut StatementLine| l.amount_minor = 101,
            |l: &mut StatementLine| l.booking_date += 1,
            |l: &mut StatementLine| l.reference.push('B'),
            |l: &mut StatementLine| l.line_ref.push('2'),
        ] {
            let mut changed = line.clone();
            change(&mut changed);
            assert_ne!(changed.digest(), base);
        }
    }
}
