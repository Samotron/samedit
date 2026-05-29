//! Org timestamp domain types and parser.
//!
//! Covers the v0.12 subset: active (`<...>`) and inactive (`[...]`) timestamps,
//! date-only and date-with-time, time ranges (`09:00-10:00`), inter-bracket
//! ranges (`<a>--<b>`), and the repeater / delay cookies (`+1w`, `++1d`,
//! `.+2d`, `-1d`).
//!
//! We parse timestamps ourselves rather than via orgize: orgize 0.9's timestamp
//! parser silently *drops* any stamp carrying a repeater or delay cookie (it
//! expects the closing bracket immediately after the date/time), which would
//! lose every repeating SCHEDULED the agenda depends on. The grammar is small
//! and well specified, so a focused parser is both correct and hermetic.

/// A calendar date (`year-month-day`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct OrgDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl OrgDate {
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        OrgDate { year, month, day }
    }
}

/// A wall-clock time of day (`hour:minute`), 24-hour.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct OrgTime {
    pub hour: u8,
    pub minute: u8,
}

impl OrgTime {
    pub fn new(hour: u8, minute: u8) -> Self {
        OrgTime { hour, minute }
    }
}

/// An Org timestamp.
///
/// Mirrors the plan's `Timestamp { date, time, repeater, is_active }`, extended
/// with the optional range end and the delay cookie so range and warning-period
/// timestamps round-trip without loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timestamp {
    /// Start date.
    pub date: OrgDate,
    /// Start time, if the timestamp carries one.
    pub time: Option<OrgTime>,
    /// End date for a range timestamp (`<a>--<b>` or a same-day time range).
    pub end_date: Option<OrgDate>,
    /// End time for a range timestamp, else `None`.
    pub end_time: Option<OrgTime>,
    /// Repeater cookie verbatim (`+1w`, `++1d`, `.+2d`), without the leading
    /// space.
    pub repeater: Option<String>,
    /// Warning/delay cookie verbatim (`-1d`, `--2d`).
    pub delay: Option<String>,
    /// `true` for active (`<...>`), `false` for inactive (`[...]`).
    pub is_active: bool,
}

impl Timestamp {
    /// `true` if this timestamp carries a repeater cookie.
    pub fn is_repeating(&self) -> bool {
        self.repeater.is_some()
    }

    /// `true` if this is a range timestamp (`<a>--<b>` or a same-day time
    /// range).
    pub fn is_range(&self) -> bool {
        self.end_date.is_some()
    }

    /// Render a single (non-range) timestamp back to Org syntax, recomputing the
    /// weekday from the (possibly shifted) date:
    /// `<YYYY-MM-DD Day[ HH:MM][ repeater][ delay]>`. Range timestamps are
    /// formatted by start only — the bump path never serialises a range.
    pub fn format(&self) -> String {
        let (open, close) = if self.is_active {
            ('<', '>')
        } else {
            ('[', ']')
        };
        let OrgDate { year, month, day } = self.date;
        let mut s = format!(
            "{open}{year:04}-{month:02}-{day:02} {dn}",
            dn = crate::date::weekday_abbr(self.date)
        );
        if let Some(t) = self.time {
            s.push_str(&format!(" {:02}:{:02}", t.hour, t.minute));
        }
        if let Some(r) = &self.repeater {
            s.push(' ');
            s.push_str(r);
        }
        if let Some(de) = &self.delay {
            s.push(' ');
            s.push_str(de);
        }
        s.push(close);
        s
    }
}

/// Parse a single Org timestamp from the start of `s`. Returns `None` if `s`
/// does not begin with a `<` or `[` bracket (after trimming) or the bracket
/// contents are malformed.
pub fn parse_timestamp(s: &str) -> Option<Timestamp> {
    let s = s.trim_start();
    let (active, close) = match s.as_bytes().first()? {
        b'<' => (true, '>'),
        b'[' => (false, ']'),
        _ => return None,
    };

    let rest = &s[1..];
    let close_idx = rest.find(close)?;
    let inner = parse_bracket(&rest[..close_idx])?;
    let after = rest[close_idx + 1..].trim_start();

    // Inter-bracket range: `<a>--<b>` / `[a]--[b]`.
    let (end_date, end_time) = if let Some(tail) = after.strip_prefix("--") {
        let tail = tail.trim_start();
        let opener = if active { '<' } else { '[' };
        match tail.strip_prefix(opener) {
            Some(body2) => {
                let ci = body2.find(close)?;
                let inner2 = parse_bracket(&body2[..ci])?;
                (Some(inner2.date), inner2.time)
            }
            None => (None, None),
        }
    } else if inner.end_time.is_some() {
        // Same-day time range, e.g. `<2026-06-02 Tue 09:00-10:00>`.
        (Some(inner.date), inner.end_time)
    } else {
        (None, None)
    };

    Some(Timestamp {
        date: inner.date,
        time: inner.time,
        end_date,
        end_time,
        repeater: inner.repeater,
        delay: inner.delay,
        is_active: active,
    })
}

struct Bracket {
    date: OrgDate,
    time: Option<OrgTime>,
    end_time: Option<OrgTime>,
    repeater: Option<String>,
    delay: Option<String>,
}

fn parse_bracket(body: &str) -> Option<Bracket> {
    let mut tokens = body.split_whitespace();
    let date = parse_date(tokens.next()?)?;

    let mut time = None;
    let mut end_time = None;
    let mut repeater = None;
    let mut delay = None;

    for tok in tokens {
        if tok.contains(':') {
            // A clock time, possibly a `HH:MM-HH:MM` range.
            if let Some((a, b)) = tok.split_once('-')
                && a.contains(':')
            {
                time = parse_time(a);
                end_time = parse_time(b);
                continue;
            }
            time = parse_time(tok);
        } else if tok.starts_with('+') || tok.starts_with(".+") {
            repeater = Some(tok.to_string());
        } else if tok.starts_with('-') {
            delay = Some(tok.to_string());
        }
        // Anything else (the day name, e.g. `Mon`) is ignored.
    }

    Some(Bracket {
        date,
        time,
        end_time,
        repeater,
        delay,
    })
}

fn parse_date(tok: &str) -> Option<OrgDate> {
    let mut parts = tok.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(OrgDate { year, month, day })
}

fn parse_time(tok: &str) -> Option<OrgTime> {
    let (h, m) = tok.split_once(':')?;
    Some(OrgTime {
        hour: h.parse().ok()?,
        minute: m.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_only_active() {
        let ts = parse_timestamp("<2026-06-01 Mon>").unwrap();
        assert_eq!(ts.date, OrgDate::new(2026, 6, 1));
        assert_eq!(ts.time, None);
        assert!(ts.is_active);
        assert!(!ts.is_repeating());
    }

    #[test]
    fn date_with_time() {
        let ts = parse_timestamp("<2026-06-01 Mon 09:30>").unwrap();
        assert_eq!(ts.time, Some(OrgTime::new(9, 30)));
    }

    #[test]
    fn inactive_timestamp() {
        let ts = parse_timestamp("[2026-05-29 Fri 11:24]").unwrap();
        assert!(!ts.is_active);
        assert_eq!(ts.date, OrgDate::new(2026, 5, 29));
        assert_eq!(ts.time, Some(OrgTime::new(11, 24)));
    }

    #[test]
    fn repeater_preserved() {
        let ts = parse_timestamp("<2026-06-01 Mon +1w>").unwrap();
        assert_eq!(ts.repeater.as_deref(), Some("+1w"));
        assert!(ts.is_repeating());
    }

    #[test]
    fn cumulative_and_restart_repeaters() {
        assert_eq!(
            parse_timestamp("<2026-06-01 Mon ++1m>")
                .unwrap()
                .repeater
                .as_deref(),
            Some("++1m")
        );
        assert_eq!(
            parse_timestamp("<2026-06-01 Mon .+2d>")
                .unwrap()
                .repeater
                .as_deref(),
            Some(".+2d")
        );
    }

    #[test]
    fn warning_delay_cookie() {
        let ts = parse_timestamp("<2026-06-01 Mon -2d>").unwrap();
        assert_eq!(ts.delay.as_deref(), Some("-2d"));
    }

    #[test]
    fn same_day_time_range() {
        let ts = parse_timestamp("<2026-06-02 Tue 09:00-10:00>").unwrap();
        assert_eq!(ts.time, Some(OrgTime::new(9, 0)));
        assert_eq!(ts.end_time, Some(OrgTime::new(10, 0)));
        assert_eq!(ts.end_date, Some(OrgDate::new(2026, 6, 2)));
    }

    #[test]
    fn inter_bracket_range() {
        let ts = parse_timestamp("<2026-06-02 Tue 09:00>--<2026-06-03 Wed 10:00>").unwrap();
        assert_eq!(ts.date, OrgDate::new(2026, 6, 2));
        assert_eq!(ts.end_date, Some(OrgDate::new(2026, 6, 3)));
        assert_eq!(ts.end_time, Some(OrgTime::new(10, 0)));
    }

    #[test]
    fn rejects_non_timestamp() {
        assert!(parse_timestamp("not a stamp").is_none());
    }
}
