//! Calendar arithmetic on [`OrgDate`], with no external date crate.
//!
//! Uses Howard Hinnant's well-known `days_from_civil` / `civil_from_days`
//! algorithms (proleptic Gregorian) to convert dates to and from a day count,
//! which makes day differences, day/week/month/year shifts, and weekday lookup
//! straightforward. The agenda (today / next-7) and the repeater bump
//! (M12.5b) are built on these.

use crate::timestamp::OrgDate;

/// Abbreviated weekday names as Org writes them, Monday-first.
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Days since the Unix epoch (1970-01-01) for `date`.
pub fn days_from_civil(date: OrgDate) -> i64 {
    let (mut y, m, d) = (date.year as i64, date.month as i64, date.day as i64);
    y -= (m <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// The date `days` after the Unix epoch.
pub fn civil_from_days(days: i64) -> OrgDate {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    OrgDate {
        year: (y + (m <= 2) as i64) as i32,
        month: m as u32,
        day: d as u32,
    }
}

/// Abbreviated weekday name for `date` (e.g. `"Fri"`).
pub fn weekday_abbr(date: OrgDate) -> &'static str {
    // 1970-01-01 is a Thursday (index 3 in a Monday-first table).
    let idx = (days_from_civil(date) + 3).rem_euclid(7) as usize;
    WEEKDAYS[idx]
}

/// `a - b` in whole days.
pub fn diff_days(a: OrgDate, b: OrgDate) -> i64 {
    days_from_civil(a) - days_from_civil(b)
}

/// `date` shifted by `n` days (negative shifts backward).
pub fn add_days(date: OrgDate, n: i64) -> OrgDate {
    civil_from_days(days_from_civil(date) + n)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// `date` shifted by `n` months, clamping the day to the target month's length
/// (e.g. Jan 31 + 1 month → Feb 28/29).
pub fn add_months(date: OrgDate, n: i64) -> OrgDate {
    let total = (date.year as i64) * 12 + (date.month as i64 - 1) + n;
    let year = total.div_euclid(12) as i32;
    let month = (total.rem_euclid(12) + 1) as u32;
    let day = date.day.min(last_day_of_month(year, month));
    OrgDate { year, month, day }
}

/// `date` shifted by `n` years (clamping Feb 29 onto non-leap years).
pub fn add_years(date: OrgDate, n: i64) -> OrgDate {
    add_months(date, n * 12)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> OrgDate {
        OrgDate::new(y, m, day)
    }

    #[test]
    fn round_trip_civil_days() {
        for date in [
            d(1970, 1, 1),
            d(2026, 5, 29),
            d(2000, 2, 29),
            d(1969, 12, 31),
        ] {
            assert_eq!(civil_from_days(days_from_civil(date)), date);
        }
    }

    #[test]
    fn epoch_is_thursday() {
        assert_eq!(weekday_abbr(d(1970, 1, 1)), "Thu");
        assert_eq!(weekday_abbr(d(2026, 5, 29)), "Fri");
        assert_eq!(weekday_abbr(d(2026, 6, 1)), "Mon");
    }

    #[test]
    fn day_and_week_shifts() {
        assert_eq!(add_days(d(2026, 5, 29), 7), d(2026, 6, 5));
        assert_eq!(diff_days(d(2026, 6, 5), d(2026, 5, 29)), 7);
        assert_eq!(add_days(d(2026, 1, 1), -1), d(2025, 12, 31));
    }

    #[test]
    fn month_shift_clamps_day() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 28));
        assert_eq!(add_months(d(2024, 1, 31), 1), d(2024, 2, 29)); // leap year
        assert_eq!(add_months(d(2026, 12, 15), 1), d(2027, 1, 15));
    }

    #[test]
    fn year_shift_clamps_leap_day() {
        assert_eq!(add_years(d(2024, 2, 29), 1), d(2025, 2, 28));
    }
}
