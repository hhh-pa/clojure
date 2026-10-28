// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S6 default data readers: `inst`/`uuid` payload validation.
//!
//! The reader validates string payloads read-side (spanful errors)
//! while the evaluator's default table reuses the same checks for
//! direct calls; both layers report the JVM texts verbatim (oracle
//! `docs/zero-oos-s6-datareaders/probes/datareaders.out`, labels
//! `d-inst-*` / `d-uuid-*`). Non-string payloads never reach here:
//! the reader defers them for the eval-side type dispatch (nil is
//! the NPE, anything else fails the `CharSequence` cast — except
//! `uuid`, whose non-strings fail as
//! `IllegalArgumentException "#uuid data reader expected string").

/// A default-reader validation failure: JVM simple class name (the
/// reader reports it via `jvm_class()`, the evaluator maps it with
/// `throw_class_for`) plus the verbatim message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagError {
    /// Simple class name (`RuntimeException`,
    /// `IllegalArgumentException`, `NumberFormatException`).
    pub class: &'static str,
    /// The verbatim JVM message.
    pub message: String,
}

impl TagError {
    fn runtime(message: String) -> Self {
        Self {
            class: "RuntimeException",
            message,
        }
    }

    fn iae(message: String) -> Self {
        Self {
            class: "IllegalArgumentException",
            message,
        }
    }

    fn nfe(message: String) -> Self {
        Self {
            class: "NumberFormatException",
            message,
        }
    }
}

/// Validate an `inst` string payload: the `clojure.instant` grammar
/// (`YYYY(-MM(-DD(THH:MM(:SS(.frac)?)?)?)?)?` with padded
/// components and uppercase `T`, plus an optional `Z`/`[+-]HH:MM`
/// zone on any prefix) and the six range asserts (leap-aware days,
/// leap-second 60 iff minutes is 59). Years take any four digits
/// (no assert). Success means the payload reads (as a
/// `TaggedLiteral` until the date/time value-kind slice lands).
pub fn validate_inst(s: &str) -> Result<(), TagError> {
    let parts = parse_instant(s)
        .ok_or_else(|| TagError::runtime(format!("Unrecognized date/time syntax: {s}")))?;
    check_ranges(&parts)
}

/// Validate a `uuid` string payload: `UUID.fromString` semantics
/// (length cap, five dash-separated parts of unchecked lengths,
/// non-empty all-hex — lengths are lenient, probe
/// `d-uuid-shortgroup`).
pub fn validate_uuid(s: &str) -> Result<(), TagError> {
    // The length gate runs first, in UTF-16 units like the JDK
    // (probes `d-uuid-braces`, `d-uuid-urn`).
    if s.encode_utf16().count() > 36 {
        return Err(TagError::iae("UUID string too large".to_owned()));
    }
    // Plain split: trailing empties are kept (unlike Java's default
    // split — probes `d-uuid-traildash`, `d-uuid-dashes` show the
    // empty part error, which needs all five slots).
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return Err(TagError::iae(format!("Invalid UUID string: {s}")));
    }
    for part in parts {
        if part.is_empty() {
            return Err(TagError::nfe(
                "For input string: \"\" under radix 16".to_owned(),
            ));
        }
        // Left-to-right, first bad digit wins (probe
        // `d-uuid-badthenempty`); the index counts chars (equal to
        // the JDK's UTF-16 index for BMP input).
        if let Some(idx) = part.chars().position(|c| !c.is_ascii_hexdigit()) {
            return Err(TagError::nfe(format!(
                "Error at index {idx} in: \"{part}\""
            )));
        }
    }
    Ok(())
}

/// One parsed instant: present components only (year is required).
struct InstantParts {
    year: u32,
    month: Option<u32>,
    day: Option<u32>,
    hour: Option<u32>,
    minute: Option<u32>,
    second: Option<u32>,
    offset_hour: Option<u32>,
    offset_minute: Option<u32>,
}

/// Read `n` ASCII digits as a number (`None` past the end or on a
/// non-digit).
fn take_digits(bytes: &[u8], pos: &mut usize, n: usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..n {
        let c = *bytes.get(*pos)?;
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(c - b'0');
        *pos += 1;
    }
    Some(value)
}

/// Consume one literal byte (`false` leaves the cursor).
fn take_lit(bytes: &[u8], pos: &mut usize, c: u8) -> bool {
    if bytes.get(*pos) == Some(&c) {
        *pos += 1;
        true
    } else {
        false
    }
}

/// Parse the instant grammar (`None` ⇒ the syntax error).
fn parse_instant(s: &str) -> Option<InstantParts> {
    let bytes = s.as_bytes();
    let mut pos = 0;
    let year = take_digits(bytes, &mut pos, 4)?;
    let month = take_opt2(bytes, &mut pos)?;
    let day = if month.is_some() {
        take_opt2(bytes, &mut pos)?
    } else {
        None
    };
    let (hour, minute, second) = parse_time(bytes, &mut pos, day.is_some())?;
    let (offset_hour, offset_minute) = parse_zone(bytes, &mut pos);
    if pos != bytes.len() {
        return None;
    }
    Some(InstantParts {
        year,
        month,
        day,
        hour,
        minute,
        second,
        offset_hour,
        offset_minute,
    })
}

/// Parse an optional `-DD` component: outer `None` fails the
/// whole grammar (a dash without digits), inner `None` means
/// absent (no dash).
fn take_opt2(bytes: &[u8], pos: &mut usize) -> Option<Option<u32>> {
    if take_lit(bytes, pos, b'-') {
        Some(Some(take_digits(bytes, pos, 2)?))
    } else {
        Some(None)
    }
}

/// Parse the optional time (`T` requires the day; `None` ⇒ the
/// syntax error).
fn parse_time(
    bytes: &[u8],
    pos: &mut usize,
    has_day: bool,
) -> Option<(Option<u32>, Option<u32>, Option<u32>)> {
    if !has_day || !take_lit(bytes, pos, b'T') {
        return Some((None, None, None));
    }
    let hour = take_digits(bytes, pos, 2)?;
    if !take_lit(bytes, pos, b':') {
        return None;
    }
    let minute = take_digits(bytes, pos, 2)?;
    let second = if take_lit(bytes, pos, b':') {
        let sec = take_digits(bytes, pos, 2)?;
        if take_lit(bytes, pos, b'.') {
            // Any digit run (millis truncation needs no value);
            // the dot alone fails (probe `d-inst-dotnofrac`).
            take_digits(bytes, pos, 1)?;
            while bytes.get(*pos).is_some_and(|c| c.is_ascii_digit()) {
                *pos += 1;
            }
        }
        Some(sec)
    } else {
        None
    };
    Some((Some(hour), Some(minute), second))
}

/// Parse the optional zone (`Z` needs no range check; a sign needs
/// `HH:MM` with a mandatory colon). A broken zone restores the
/// cursor so the trailing sign always fails the grammar (without
/// the restore, `"2024+02"` would read zone-less at end-of-input).
fn parse_zone(bytes: &[u8], pos: &mut usize) -> (Option<u32>, Option<u32>) {
    if take_lit(bytes, pos, b'Z') {
        return (None, None);
    }
    let save = *pos;
    if take_lit(bytes, pos, b'+') || take_lit(bytes, pos, b'-') {
        if let Some(h) = take_digits(bytes, pos, 2) {
            if take_lit(bytes, pos, b':') {
                if let Some(m) = take_digits(bytes, pos, 2) {
                    return (Some(h), Some(m));
                }
            }
        }
        *pos = save;
    }
    (None, None)
}

/// Whether `year` is a leap year (proleptic Gregorian).
fn is_leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days in `month` (1-based, validated before this runs).
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Check the parsed ranges in source order (months before days —
/// probe `d-inst-badday`).
fn check_ranges(parts: &InstantParts) -> Result<(), TagError> {
    check_date_ranges(parts)?;
    check_time_ranges(parts)?;
    check_zone_ranges(parts)?;
    Ok(())
}

/// Months, then days (leap-aware).
fn check_date_ranges(parts: &InstantParts) -> Result<(), TagError> {
    if let Some(m) = parts.month {
        if !(1..=12).contains(&m) {
            return Err(TagError::runtime("failed: (<= 1 months 12)".to_owned()));
        }
    }
    if let Some(d) = parts.day {
        let dim = days_in_month(parts.year, parts.month.unwrap_or(1));
        if d < 1 || d > dim {
            return Err(TagError::runtime(
                "failed: (<= 1 days (days-in-month months (leap-year? years)))".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Hours, minutes, then seconds (leap-second 60 reads iff minutes
/// is 59 — probe `d-inst-leapsec`; the message names the rule
/// verbatim).
fn check_time_ranges(parts: &InstantParts) -> Result<(), TagError> {
    if parts.hour.is_some_and(|h| h > 23) {
        return Err(TagError::runtime("failed: (<= 0 hours 23)".to_owned()));
    }
    if parts.minute.is_some_and(|m| m > 59) {
        return Err(TagError::runtime("failed: (<= 0 minutes 59)".to_owned()));
    }
    if let Some(s) = parts.second {
        let max = if parts.minute == Some(59) { 60 } else { 59 };
        if s > max {
            return Err(TagError::runtime(
                "failed: (<= 0 seconds (if (= minutes 59) 60 59))".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Zone offset hours, then minutes.
fn check_zone_ranges(parts: &InstantParts) -> Result<(), TagError> {
    if parts.offset_hour.is_some_and(|h| h > 23) {
        return Err(TagError::runtime(
            "failed: (<= 0 offset-hours 23)".to_owned(),
        ));
    }
    if parts.offset_minute.is_some_and(|m| m > 59) {
        return Err(TagError::runtime(
            "failed: (<= 0 offset-minutes 59)".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TagError, validate_inst, validate_uuid};

    fn err_of(result: Result<(), TagError>) -> (String, String) {
        let err = result.expect_err("expected a tag error");
        (err.class.to_owned(), err.message)
    }

    #[test]
    fn inst_accepts_prefixes_zones_and_fraction() {
        // Every valid shape (probes `d-inst`, `d-inst-offset`,
        // `d-inst-nozone`, `d-inst-ym`, `d-inst-year`,
        // `d-inst-nosecs`, `d-inst-zonez`, `d-inst-frac7`,
        // `d-inst-leapsec`, `d-inst-yearzone`, `d-inst-monthzone`,
        // `d-inst-dayzone`, plus leap edge years).
        for s in [
            "2024-01-01",
            "2024-01-01T10:00:00.000+02:00",
            "2024-01-01T00:00:00",
            "2024-01",
            "2024",
            "2024-01-01T00:00",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00.1234567",
            "2024-01-01T00:59:60",
            "2024Z",
            "2024-01Z",
            "2024-01-01+02:00",
            "2024-02-29",
            "2000-02-29",
            "2023-02-28",
            "2024-04-30",
            "2024-01-31",
        ] {
            assert!(validate_inst(s).is_ok(), "{s}");
        }
    }

    #[test]
    fn inst_rejects_grammar() {
        // Every syntax failure (probes `d-inst-bad`,
        // `d-inst-trail`, `d-inst-empty`, `d-inst-dotnofrac`,
        // `d-inst-zonenocolon`, `d-inst-lower-t`, `d-inst-lower-z`,
        // `d-inst-space`, `d-inst-nopad`, plus cursor edges).
        for s in [
            "bogus",
            "2024-01-01xyz",
            "",
            "2024-01-01T00:00:00.",
            "2024-01-01T00:00:00+0200",
            "2024-01-01t00:00:00z",
            "2024-01-01T00:00:00z",
            "2024-01-01 00:00:00",
            "2024-1-1",
            "20x4",
            "202",
            "2024-",
            "2024-01-01T",
            "2024-01-01Txx",
            "2024-01-01T0000",
            "2024-01-01T00:xx",
            "2024-01-01T00:00:xx",
            "2024+02",
            "2024+",
            "2024+02:",
            "2024-01T00:00",
        ] {
            assert_eq!(
                err_of(validate_inst(s)),
                (
                    "RuntimeException".to_owned(),
                    format!("Unrecognized date/time syntax: {s}")
                ),
                "{s}"
            );
        }
    }

    #[test]
    fn inst_rejects_ranges() {
        // Every range assert (probes `d-inst-badday`,
        // `d-inst-day45`, `d-inst-feb29`, `d-inst-hour25`,
        // `d-inst-min60`, `d-inst-sec61`, `d-inst-month00`,
        // `d-inst-day00`, `d-inst-zone25`, `d-inst-zone60`, plus
        // century leap edges and the 30-day month).
        for (s, message) in [
            ("2024-13-45", "failed: (<= 1 months 12)"),
            ("2024-00-01", "failed: (<= 1 months 12)"),
            (
                "2024-01-45",
                "failed: (<= 1 days (days-in-month months (leap-year? years)))",
            ),
            (
                "2024-01-00",
                "failed: (<= 1 days (days-in-month months (leap-year? years)))",
            ),
            (
                "2023-02-29",
                "failed: (<= 1 days (days-in-month months (leap-year? years)))",
            ),
            (
                "1900-02-29",
                "failed: (<= 1 days (days-in-month months (leap-year? years)))",
            ),
            (
                "2024-04-31",
                "failed: (<= 1 days (days-in-month months (leap-year? years)))",
            ),
            ("2024-01-01T25:00:00", "failed: (<= 0 hours 23)"),
            ("2024-01-01T00:60:00", "failed: (<= 0 minutes 59)"),
            (
                "2024-01-01T00:00:61",
                "failed: (<= 0 seconds (if (= minutes 59) 60 59))",
            ),
            (
                "2024-01-01T00:58:60",
                "failed: (<= 0 seconds (if (= minutes 59) 60 59))",
            ),
            (
                "2024-01-01T00:00:00+25:00",
                "failed: (<= 0 offset-hours 23)",
            ),
            (
                "2024-01-01T00:00:00+02:60",
                "failed: (<= 0 offset-minutes 59)",
            ),
        ] {
            assert_eq!(
                err_of(validate_inst(s)),
                ("RuntimeException".to_owned(), message.to_owned()),
                "{s}"
            );
        }
    }

    #[test]
    fn uuid_accepts_lenient_lengths() {
        // Group lengths are unchecked (probes `d-uuid`,
        // `d-uuid-upper`, `d-uuid-shortgroup`, `d-uuid-shortmid`,
        // `d-uuid-longshort`).
        for s in [
            "550e8400-e29b-41d4-a716-446655440000",
            "550E8400-E29B-41D4-A716-446655440000",
            "550e840-e29b-41d4-a716-446655440000",
            "550e8400-e29b-41d-a716-446655440000",
            "123456789-123-1234-1234-123456789012",
        ] {
            assert!(validate_uuid(s).is_ok(), "{s}");
        }
    }

    #[test]
    fn uuid_rejects_shapes() {
        // Length cap, part count, empty parts, bad digits (probes
        // `d-uuid-bad`, `d-uuid-short`, `d-uuid-empty`,
        // `d-uuid-braces`, `d-uuid-urn`, `d-uuid-nonhex`,
        // `d-uuid-nodash`, `d-uuid-36nodash`, `d-uuid-emptypart`,
        // `d-uuid-badidx0`, `d-uuid-traildash`, `d-uuid-dashes`,
        // `d-uuid-badthenempty`).
        assert_eq!(
            err_of(validate_uuid("{550e8400-e29b-41d4-a716-446655440000}")),
            (
                "IllegalArgumentException".to_owned(),
                "UUID string too large".to_owned()
            )
        );
        assert_eq!(
            err_of(validate_uuid(
                "urn:uuid:550e8400-e29b-41d4-a716-446655440000"
            )),
            (
                "IllegalArgumentException".to_owned(),
                "UUID string too large".to_owned()
            )
        );
        for s in [
            "bogus",
            "550e8400-e29b-41d4-a716",
            "",
            "550e8400e29b41d4a716446655440000",
            "550e8400e29b41d4a71644665544000000",
        ] {
            assert_eq!(
                err_of(validate_uuid(s)),
                (
                    "IllegalArgumentException".to_owned(),
                    format!("Invalid UUID string: {s}")
                ),
                "{s}"
            );
        }
        for s in [
            "550e8400--41d4-a716-446655440000",
            "550e8400-e29b-41d4-a716-",
            "----",
        ] {
            assert_eq!(
                err_of(validate_uuid(s)),
                (
                    "NumberFormatException".to_owned(),
                    "For input string: \"\" under radix 16".to_owned()
                ),
                "{s}"
            );
        }
        assert_eq!(
            err_of(validate_uuid("550e8400-e29b-41d4-a716-44665544000g")),
            (
                "NumberFormatException".to_owned(),
                "Error at index 11 in: \"44665544000g\"".to_owned()
            )
        );
        assert_eq!(
            err_of(validate_uuid("g50e8400-e29b-41d4-a716-446655440000")),
            (
                "NumberFormatException".to_owned(),
                "Error at index 0 in: \"g50e8400\"".to_owned()
            )
        );
        assert_eq!(
            err_of(validate_uuid("g50e8400--41d4-a716-446655440000")),
            (
                "NumberFormatException".to_owned(),
                "Error at index 0 in: \"g50e8400\"".to_owned()
            )
        );
    }
}
