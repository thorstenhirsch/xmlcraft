//! `duration`: a number of months and a number of seconds (XML Schema Part 2, 3.2.6).
//!
//! Two durations are equal when both numbers are. Their order is partial: a month is not a fixed number of days, so `P1M` and `P30D`
//! are in no order; one duration is less than another when it is less after being added to each of four reference dates.

use std::cmp::Ordering;

use chrono::{Days, NaiveDate};

use crate::decimal::Decimal;

/// The most digits a component of a duration may have.
const MAX_COMPONENT_DIGITS: usize = 18;
/// The most fraction digits of the seconds that are read.
const MAX_FRACTION_DIGITS: usize = 1000;
/// The four dates a duration is added to for ordering (XML Schema Part 2, 3.2.6.2): all on the first of a month, in a leap year, before
/// one, after one and in a year that is not.
const REFERENCES: [(i128, u8); 4] = [(1696, 9), (1697, 2), (1903, 3), (1903, 7)];
const DAYS_PER_400_YEARS: i128 = 146_097;

/// A duration value. Both numbers have the sign of the duration (or are zero).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Duration {
    months: i128,
    seconds: Decimal,
}

impl Duration {
    pub fn parse(lexical: &str) -> Option<Self> {
        let (negative, rest) = match lexical.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, lexical),
        };
        let rest = rest.strip_prefix('P')?;
        let (date_part, time_part) = match rest.split_once('T') {
            Some((d, t)) => (d, Some(t)),
            None => (rest, None),
        };
        if date_part.is_empty() && time_part.is_none_or(str::is_empty) {
            return None;
        }
        let mut seen = 0usize;
        let (years, after_years) = component(date_part, 'Y')?;
        let (months, after_months) = component(after_years, 'M')?;
        let (days, after_days) = component(after_months, 'D')?;
        seen += usize::from(years.is_some()) + usize::from(months.is_some()) + usize::from(days.is_some());
        if !after_days.is_empty() {
            return None;
        }
        let (mut hours, mut minutes, mut seconds) = (None, None, None::<(i128, String)>);
        if let Some(time) = time_part {
            let (h, after_h) = component(time, 'H')?;
            let (m, after_m) = component(after_h, 'M')?;
            hours = h;
            minutes = m;
            if !after_m.is_empty() {
                let body = after_m.strip_suffix('S')?;
                let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
                if body.contains('.') && fraction.is_empty() {
                    return None;
                }
                if whole.is_empty() || fraction.len() > MAX_FRACTION_DIGITS || !digits(whole) || !digits(fraction) {
                    return None;
                }
                seconds = Some((number(whole)?, fraction.to_owned()));
            }
            seen += usize::from(hours.is_some()) + usize::from(minutes.is_some()) + usize::from(seconds.is_some());
            if seen == 0 || time.is_empty() {
                return None;
            }
        }
        let total_months = years.unwrap_or(0).checked_mul(12)?.checked_add(months.unwrap_or(0))?;
        let (second_whole, fraction) = seconds.unwrap_or((0, String::new()));
        let whole_seconds = days
            .unwrap_or(0)
            .checked_mul(86_400)?
            .checked_add(hours.unwrap_or(0).checked_mul(3600)?)?
            .checked_add(minutes.unwrap_or(0).checked_mul(60)?)?
            .checked_add(second_whole)?;
        let seconds = Decimal::parse(&if fraction.is_empty() { whole_seconds.to_string() } else { format!("{whole_seconds}.{fraction}") })?;
        Some(if negative { Self { months: -total_months, seconds: seconds.negated() } } else { Self { months: total_months, seconds } })
    }

    pub fn months(&self) -> i128 {
        self.months
    }

    pub fn seconds(&self) -> &Decimal {
        &self.seconds
    }

    pub fn is_zero(&self) -> bool {
        self.months == 0 && self.seconds.is_zero()
    }

    pub fn equals(&self, other: &Self) -> bool {
        self == other
    }

    /// The order of two durations; `None` when it depends on the date they are added to.
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        if self == other {
            return Some(Ordering::Equal);
        }
        let mut result: Option<Ordering> = None;
        for (year, month) in REFERENCES {
            let order = self.point(year, month)?.cmp(&other.point(year, month)?);
            match result {
                None => result = Some(order),
                Some(previous) if previous == order => {}
                Some(_) => return None,
            }
        }
        // All four agree. Equal at every reference date is not the same value (`P1M` and `P31D` are equal on 1 Jan 1903): not ordered.
        result.filter(|order| *order != Ordering::Equal)
    }

    /// The date and time that is this duration after the first of `month` of `year` (astronomical years), as a key that sorts in time.
    fn point(&self, year: i128, month: u8) -> Option<(i128, u8, u8, u32, Decimal)> {
        let total = year.checked_mul(12)?.checked_add(i128::from(month) - 1)?.checked_add(self.months)?;
        let mut year = total.div_euclid(12);
        let month = u8::try_from(total.rem_euclid(12) + 1).ok()?;
        // The seconds, split into whole seconds rounded down and a fraction that is not negative.
        let (whole, fraction) = split_seconds(&self.seconds)?;
        let days = whole.div_euclid(86_400);
        let second_of_day = u32::try_from(whole.rem_euclid(86_400)).ok()?;
        year = year.checked_add(days.div_euclid(DAYS_PER_400_YEARS).checked_mul(400)?)?;
        let rest = u64::try_from(days.rem_euclid(DAYS_PER_400_YEARS)).ok()?;
        // Within chrono's range, on a year that has the calendar of this one.
        let base = i32::try_from(year.rem_euclid(400)).ok()?;
        let date = NaiveDate::from_ymd_opt(2000 + base, u32::from(month), 1)?.checked_add_days(Days::new(rest))?;
        let year = year - i128::from(base) + i128::from(chrono::Datelike::year(&date) - 2000);
        Some((year, u8::try_from(chrono::Datelike::month(&date)).ok()?, u8::try_from(chrono::Datelike::day(&date)).ok()?, second_of_day, fraction))
    }

    /// The canonical lexical representation: years and months from the months, days, hours, minutes and seconds from the seconds.
    pub fn canonical(&self) -> String {
        if self.is_zero() {
            return "PT0S".to_owned();
        }
        let negative = self.months < 0 || self.seconds.is_negative();
        let months = self.months.unsigned_abs();
        let seconds = if self.seconds.is_negative() { self.seconds.negated() } else { self.seconds.clone() };
        let whole: u128 = seconds.integer_digits().parse().unwrap_or(0);
        let (days, hours, minutes, secs) = (whole / 86_400, whole % 86_400 / 3600, whole % 3600 / 60, whole % 60);
        let mut out = String::from(if negative { "-P" } else { "P" });
        for (value, unit) in [(months / 12, 'Y'), (months % 12, 'M'), (days, 'D')] {
            if value != 0 {
                out.push_str(&format!("{value}{unit}"));
            }
        }
        if hours != 0 || minutes != 0 || secs != 0 || !seconds.is_integer() {
            out.push('T');
            for (value, unit) in [(hours, 'H'), (minutes, 'M')] {
                if value != 0 {
                    out.push_str(&format!("{value}{unit}"));
                }
            }
            if secs != 0 || !seconds.is_integer() {
                out.push_str(&secs.to_string());
                if !seconds.is_integer() {
                    out.push('.');
                    out.push_str(seconds.fraction_part());
                }
                out.push('S');
            }
        }
        out
    }
}

fn digits(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_digit())
}

fn number(text: &str) -> Option<i128> {
    if text.len() > MAX_COMPONENT_DIGITS { None } else { text.parse().ok() }
}

/// A leading `nX` of `text`: the number and what follows; `(None, text)` when `text` does not start that way.
fn component(text: &str, unit: char) -> Option<(Option<i128>, &str)> {
    let end = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    match text.get(end..).and_then(|rest| rest.strip_prefix(unit)) {
        Some(after) if end > 0 => Some((Some(number(text.get(..end)?)?), after)),
        Some(_) => None,
        None => Some((None, text)),
    }
}

/// The seconds as whole seconds rounded down and a fraction that is not negative.
fn split_seconds(seconds: &Decimal) -> Option<(i128, Decimal)> {
    let whole: i128 = seconds.integer_digits().parse().ok()?;
    let fraction = seconds.fraction_part();
    if !seconds.is_negative() {
        return Some((whole, Decimal::parse(&format!("0.{fraction}")).unwrap_or_else(Decimal::zero)));
    }
    if fraction.is_empty() {
        return Some((-whole, Decimal::zero()));
    }
    // -5.25 is -6 and 0.75.
    Some((-whole - 1, Decimal::parse(&format!("0.{}", ten_complement(fraction)))?))
}

/// `10^n - f` for the `n` digits `f`, as `n` digits.
fn ten_complement(fraction: &str) -> String {
    let mut digits: Vec<u8> = fraction.bytes().map(|b| 9 - (b - b'0')).collect();
    for digit in digits.iter_mut().rev() {
        if *digit == 9 {
            *digit = 0;
        } else {
            *digit += 1;
            break;
        }
    }
    digits.into_iter().map(|d| char::from(b'0' + d)).collect()
}
