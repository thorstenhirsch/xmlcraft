//! The date and time types: `dateTime`, `date`, `time`, `gYearMonth`, `gYear`, `gMonthDay`, `gDay`, `gMonth`.
//!
//! A value is the seven properties of the schema language: year, month, day, hour, minute, second (with a fraction) and an optional time
//! zone offset. A type that does not have a property gets the default that makes the order of its values the order of the periods they
//! name (a `gYear` starts on 1 January, a `gDay` is a day of December 1972, a `time` is a time on 31 December 1972, a `gMonthDay` is in
//! 1972, a leap year).
//!
//! Calendar arithmetic is chrono's. Years go beyond what chrono holds (the schema language allows any number of digits; up to 18 are
//! read here): a date is moved by whole 400-year cycles of the Gregorian calendar, after which the calendar repeats, into chrono's range,
//! worked out there, and moved back.

use std::cmp::Ordering;

use chrono::{Datelike, NaiveDate, TimeDelta, Timelike};

use crate::decimal::Decimal;

/// Which of the eight types a value belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    DateTime,
    Date,
    Time,
    GYearMonth,
    GYear,
    GMonthDay,
    GDay,
    GMonth,
}

/// The most digits of a year that are read.
const MAX_YEAR_DIGITS: usize = 18;
/// The most fraction digits of a second that are read.
const MAX_FRACTION_DIGITS: usize = 1000;
/// The default year for what has none (a leap year, so that `--02-29` is a date).
const DEFAULT_YEAR: i64 = 1972;
/// A time zone offset may be as large as this, in minutes (`14:00`).
const MAX_OFFSET: i32 = 14 * 60;

/// A date or time value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Temporal {
    kind: Kind,
    /// The year as the schema language counts them: there is no year 0, 1 BCE is `-1`.
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: Decimal,
    /// The offset from UTC in minutes, `Some(0)` for `Z`.
    offset: Option<i32>,
}

/// The properties compared, in order of significance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Fields {
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: Decimal,
}

/// The year as astronomical year numbering counts (year 0 is 1 BCE).
fn astronomical(year: i64) -> i64 {
    if year < 0 { year + 1 } else { year }
}

fn from_astronomical(year: i64) -> i64 {
    if year <= 0 { year - 1 } else { year }
}

/// A year inside chrono's range that has the same calendar as `year` (a multiple of 400 years apart), and the years moved.
fn shifted(year: i64) -> (i32, i64) {
    let astro = astronomical(year);
    let base = astro.rem_euclid(400);
    // 2000 + (0..400) is far inside chrono's range.
    (2000 + i32::try_from(base).unwrap_or(0), astro.div_euclid(400))
}

fn days_in_month(year: i64, month: u8) -> u8 {
    let (y, _) = shifted(year);
    (28..=31).rev().find(|d| NaiveDate::from_ymd_opt(y, u32::from(month), u32::from(*d)).is_some()).unwrap_or(28)
}

/// The day after the given one.
fn next_day(year: i64, month: u8, day: u8) -> Option<(i64, u8, u8)> {
    let (y, cycles) = shifted(year);
    let next = NaiveDate::from_ymd_opt(y, u32::from(month), u32::from(day))?.succ_opt()?;
    let astro = cycles.checked_mul(400)?.checked_add(i64::from(next.year()) - 2000)?;
    Some((from_astronomical(astro), u8::try_from(next.month()).ok()?, u8::try_from(next.day()).ok()?))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        let yes = self.peek() == Some(b);
        if yes {
            self.at += 1;
        }
        yes
    }

    fn expect(&mut self, b: u8) -> Option<()> {
        self.eat(b).then_some(())
    }

    /// Exactly two digits.
    fn two(&mut self) -> Option<u8> {
        let (a, b) = (self.peek()?, self.bytes.get(self.at + 1).copied()?);
        if a.is_ascii_digit() && b.is_ascii_digit() {
            self.at += 2;
            Some((a - b'0') * 10 + (b - b'0'))
        } else {
            None
        }
    }

    /// `-?` and at least four digits (no leading zero beyond four); not zero.
    fn year(&mut self) -> Option<i64> {
        let negative = self.eat(b'-');
        let start = self.at;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.at += 1;
        }
        let digits = self.bytes.get(start..self.at)?;
        if digits.len() < 4 || digits.len() > MAX_YEAR_DIGITS || (digits.len() > 4 && digits.first() == Some(&b'0')) {
            return None;
        }
        let value: i64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
        (value != 0).then_some(if negative { -value } else { value })
    }

    /// `ss` or `ss.s+`.
    fn second(&mut self) -> Option<Decimal> {
        let whole = self.two()?;
        let mut text = whole.to_string();
        if self.eat(b'.') {
            let start = self.at;
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.at += 1;
            }
            let fraction = std::str::from_utf8(self.bytes.get(start..self.at)?).ok()?;
            if fraction.is_empty() || fraction.len() > MAX_FRACTION_DIGITS {
                return None;
            }
            text.push('.');
            text.push_str(fraction);
        }
        Decimal::parse(&text)
    }

    /// `Z`, or `+hh:mm` / `-hh:mm` up to fourteen hours.
    fn offset(&mut self) -> Option<Option<i32>> {
        if self.eat(b'Z') {
            return Some(Some(0));
        }
        let sign = match self.peek() {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => return Some(None),
        };
        self.at += 1;
        let hours = i32::from(self.two()?);
        self.expect(b':')?;
        let minutes = i32::from(self.two()?);
        let total = hours * 60 + minutes;
        (minutes < 60 && total <= MAX_OFFSET).then_some(Some(sign * total))
    }
}

impl Temporal {
    /// Read `lexical` as a value of `kind`.
    pub fn parse(kind: Kind, lexical: &str) -> Option<Self> {
        let mut c = Cursor { bytes: lexical.as_bytes(), at: 0 };
        let mut v = Self {
            kind,
            year: DEFAULT_YEAR,
            month: match kind {
                Kind::Time | Kind::GDay => 12,
                _ => 1,
            },
            day: if kind == Kind::Time { 31 } else { 1 },
            hour: 0,
            minute: 0,
            second: Decimal::zero(),
            offset: None,
        };
        let time_of_day = |c: &mut Cursor<'_>| -> Option<(u8, u8, Decimal)> {
            let hour = c.two()?;
            c.expect(b':')?;
            let minute = c.two()?;
            c.expect(b':')?;
            Some((hour, minute, c.second()?))
        };
        match kind {
            Kind::DateTime | Kind::Date | Kind::GYearMonth | Kind::GYear => {
                v.year = c.year()?;
                if kind != Kind::GYear {
                    c.expect(b'-')?;
                    v.month = c.two()?;
                }
                if matches!(kind, Kind::DateTime | Kind::Date) {
                    c.expect(b'-')?;
                    v.day = c.two()?;
                }
                if kind == Kind::DateTime {
                    c.expect(b'T')?;
                    (v.hour, v.minute, v.second) = time_of_day(&mut c)?;
                }
            }
            Kind::Time => (v.hour, v.minute, v.second) = time_of_day(&mut c)?,
            Kind::GMonthDay => {
                c.expect(b'-')?;
                c.expect(b'-')?;
                v.month = c.two()?;
                c.expect(b'-')?;
                v.day = c.two()?;
            }
            Kind::GDay => {
                for _ in 0..3 {
                    c.expect(b'-')?;
                }
                v.day = c.two()?;
            }
            Kind::GMonth => {
                c.expect(b'-')?;
                c.expect(b'-')?;
                v.month = c.two()?;
            }
        }
        v.offset = c.offset()?;
        if c.peek().is_some() {
            return None;
        }
        v.validated()
    }

    /// The value if its properties are in range; `24:00:00` becomes `00:00:00` of the next day.
    fn validated(mut self) -> Option<Self> {
        if !(1..=12).contains(&self.month) {
            return None;
        }
        let limit = match self.kind {
            Kind::GDay => 31,
            Kind::GMonthDay => days_in_month(DEFAULT_YEAR, self.month),
            _ => days_in_month(self.year, self.month),
        };
        if !(1..=limit).contains(&self.day) || self.minute > 59 || self.second >= Decimal::from_i64(60) {
            return None;
        }
        match self.hour {
            0..=23 => {}
            24 if self.minute == 0 && self.second.is_zero() => {
                self.hour = 0;
                if matches!(self.kind, Kind::DateTime | Kind::Date) {
                    (self.year, self.month, self.day) = next_day(self.year, self.month, self.day)?;
                }
            }
            _ => return None,
        }
        Some(self)
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The time zone offset in minutes, if the value has one.
    pub fn offset(&self) -> Option<i32> {
        self.offset
    }

    /// The properties in UTC (when there is an offset), or as written.
    fn fields(&self, offset: Option<i32>) -> Option<Fields> {
        let own = Fields { year: self.year, month: self.month, day: self.day, hour: self.hour, minute: self.minute, second: self.second.clone() };
        let Some(minutes) = offset else { return Some(own) };
        let (y, cycles) = shifted(self.year);
        let moved = NaiveDate::from_ymd_opt(y, u32::from(self.month), u32::from(self.day))?
            .and_hms_opt(u32::from(self.hour), u32::from(self.minute), 0)?
            .checked_sub_signed(TimeDelta::try_minutes(i64::from(minutes))?)?;
        let astro = cycles.checked_mul(400)?.checked_add(i64::from(moved.year()) - 2000)?;
        Some(Fields {
            year: from_astronomical(astro),
            month: u8::try_from(moved.month()).ok()?,
            day: u8::try_from(moved.day()).ok()?,
            hour: u8::try_from(moved.hour()).ok()?,
            minute: u8::try_from(moved.minute()).ok()?,
            second: self.second.clone(),
        })
    }

    /// The order of two values of the same type: values with a time zone are compared in UTC; a value with a zone and one without are
    /// ordered only if they would be whatever zone the second had (within fourteen hours of UTC); otherwise they are not comparable (`None`).
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        if self.kind != other.kind {
            return None;
        }
        match (self.offset, other.offset) {
            (None, None) | (Some(_), Some(_)) => Some(self.fields(self.offset)?.cmp(&other.fields(other.offset)?)),
            (Some(_), None) => self.against_local(other),
            (None, Some(_)) => other.against_local(self).map(Ordering::reverse),
        }
    }

    /// `self` has a zone and `local` has none.
    fn against_local(&self, local: &Self) -> Option<Ordering> {
        let mine = self.fields(self.offset)?;
        if mine < local.fields(Some(MAX_OFFSET))? {
            Some(Ordering::Less)
        } else if mine > local.fields(Some(-MAX_OFFSET))? {
            Some(Ordering::Greater)
        } else {
            None
        }
    }

    /// Whether the two are the same value (not comparable is not equal).
    pub fn equals(&self, other: &Self) -> bool {
        self.compare(other) == Some(Ordering::Equal)
    }

    /// The canonical lexical representation.
    pub fn canonical(&self) -> String {
        let year = || {
            let magnitude = self.year.unsigned_abs();
            format!("{}{magnitude:04}", if self.year < 0 { "-" } else { "" })
        };
        let second = || {
            let text = self.second.to_string();
            if self.second.integer_digits().len() < 2 { format!("0{text}") } else { text }
        };
        let time = || format!("{:02}:{:02}:{}", self.hour, self.minute, second());
        let mut out = match self.kind {
            Kind::DateTime => format!("{}-{:02}-{:02}T{}", year(), self.month, self.day, time()),
            Kind::Date => format!("{}-{:02}-{:02}", year(), self.month, self.day),
            Kind::Time => time(),
            Kind::GYearMonth => format!("{}-{:02}", year(), self.month),
            Kind::GYear => year(),
            Kind::GMonthDay => format!("--{:02}-{:02}", self.month, self.day),
            Kind::GDay => format!("---{:02}", self.day),
            Kind::GMonth => format!("--{:02}", self.month),
        };
        match self.offset {
            None => {}
            Some(0) => out.push('Z'),
            Some(minutes) => {
                let sign = if minutes < 0 { '-' } else { '+' };
                out.push_str(&format!("{sign}{:02}:{:02}", minutes.abs() / 60, minutes.abs() % 60));
            }
        }
        out
    }
}
