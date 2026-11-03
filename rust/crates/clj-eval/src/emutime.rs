// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! `java.time` emulation (#205).
//!
//! The suite's date surface, nothing more: `LocalDate/of now`,
//! `LocalDateTime/of now`, `Instant/ofEpochSecond`,
//! `DateTimeFormatter/ofPattern`, the `ZoneOffset/UTC` field, and
//! the instance members `plusDays getYear getMonthValue
//! getDayOfMonth getDayOfWeek getValue format atZone toLocalDate
//! toEpochSecond`. Civil arithmetic is Howard Hinnant's
//! days-from-civil pair over `i64` day counts (verified against
//! `java.time` anchors in the cover test); validation messages
//! are the JDK texts. The port has no `DateTimeException`
//! variant, so invalid dates throw `IllegalArgumentException`
//! (the suite's only date validation catches broad `Exception`).
//! `now` reads the system clock as UTC (the suite asserts shape
//! only). Patterns support the `yMdHms` runs the suite uses;
//! anything else fails loudly at `ofPattern`.

use std::iter::Peekable;
use std::str::Chars;
use std::sync::Arc;

use clj_value::{ExtensionValue, ThrowClass, Value};

use crate::error::NativeError;

/// Whether `v` is one of ours (the dispatch guard).
fn is_time(v: &Value) -> bool {
    as_localdate(v).is_some()
        || as_datetime(v).is_some()
        || as_instant(v).is_some()
        || as_zone(v).is_some()
        || as_zoned(v).is_some()
        || as_dow(v).is_some()
        || as_formatter(v).is_some()
}

/// A `java.time` instance call, or `None` when the target is not
/// ours (the generic miss).
pub(crate) fn time_method(
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    if !is_time(target) {
        return None;
    }
    Some(time_call(meth, target, rest))
}

/// Dispatch over the seven inhabitants (the guard above already
/// established one matches).
fn time_call(meth: &str, target: &Value, rest: &[Value]) -> Result<Value, NativeError> {
    if let Some(d) = as_localdate(target) {
        return localdate_call(&d, meth, rest);
    }
    if let Some(dt) = as_datetime(target) {
        return datetime_call(&dt, meth, rest);
    }
    if let Some(i) = as_instant(target) {
        return instant_call(&i, meth, rest);
    }
    if let Some(z) = as_zoned(target) {
        return zoned_call(&z, meth, rest);
    }
    if let Some(w) = as_dow(target) {
        return dow_call(w, meth, rest);
    }
    if as_zone(target).is_some() {
        return Err(crate::types::no_method(
            meth,
            rest.len(),
            "java.time.ZoneOffset",
        ));
    }
    Err(crate::types::no_method(
        meth,
        rest.len(),
        "java.time.format.DateTimeFormatter",
    ))
}

/// A validated ISO date (validated at construction, so every
/// `EmuLocalDate` is a real date).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuLocalDate {
    y: i32,
    m: u8,
    d: u8,
}

/// A validated ISO time-of-day (whole seconds — the suite never
/// touches nanos).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuLocalTime {
    h: u8,
    mi: u8,
    s: u8,
}

/// A `java.time.LocalDateTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuLocalDateTime {
    date: EmuLocalDate,
    time: EmuLocalTime,
}

/// A `java.time.Instant` (whole seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuInstant {
    secs: i64,
}

/// A `java.time.ZoneOffset` (only UTC is constructible, but the
/// offset rides the arithmetic generally).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuZoneOffset {
    secs: i32,
}

/// A `java.time.ZonedDateTime` (local parts plus the offset they
/// were zoned with).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuZonedDateTime {
    dt: EmuLocalDateTime,
    offset_secs: i32,
}

/// A `java.time.DayOfWeek` (1 = Monday through 7 = Sunday, like
/// `getValue`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuDayOfWeek(u8);

/// A compiled `DateTimeFormatter` (the pattern plus its compiled
/// runs — equality is by the pattern).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EmuDateTimeFormatter {
    pattern: Arc<str>,
    elems: Vec<PatElem>,
}

/// One compiled pattern run (`yMdHms` with widths, else a
/// literal).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PatElem {
    Year(usize),
    Month,
    MonthPad,
    Day,
    DayPad,
    Hour,
    HourPad,
    Min,
    MinPad,
    Sec,
    SecPad,
    Lit(char),
}

/// Downcast helpers (one per inhabitant).
fn as_localdate(v: &Value) -> Option<EmuLocalDate> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuLocalDate>().copied(),
        _ => None,
    }
}

fn as_datetime(v: &Value) -> Option<EmuLocalDateTime> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuLocalDateTime>().copied(),
        _ => None,
    }
}

fn as_instant(v: &Value) -> Option<EmuInstant> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuInstant>().copied(),
        _ => None,
    }
}

fn as_zone(v: &Value) -> Option<EmuZoneOffset> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuZoneOffset>().copied(),
        _ => None,
    }
}

fn as_zoned(v: &Value) -> Option<EmuZonedDateTime> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuZonedDateTime>().copied(),
        _ => None,
    }
}

fn as_dow(v: &Value) -> Option<u8> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuDayOfWeek>().map(|w| w.0),
        _ => None,
    }
}

fn as_formatter(v: &Value) -> Option<EmuDateTimeFormatter> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuDateTimeFormatter>().cloned(),
        _ => None,
    }
}

/// Box a date.
fn date_value(d: EmuLocalDate) -> Value {
    Value::Extension(Arc::new(d))
}

/// Box a datetime.
fn datetime_value(dt: EmuLocalDateTime) -> Value {
    Value::Extension(Arc::new(dt))
}

/// Whether `y` is a leap year (proleptic Gregorian).
fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// The days in month `m` of year `y` (`m` is 1-based).
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

/// Days since 1970-01-01 (Hinnant `days_from_civil`, epoch-shifted).
fn days_since_epoch(y: i32, m: u8, d: u8) -> i64 {
    let mut y = i64::from(y);
    let m = i64::from(m);
    let d = i64::from(d);
    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The inverse: civil date for days since 1970-01-01 (Hinnant
/// `civil_from_days`, epoch-shifted).
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Uppercase month names (the `Invalid date` message).
const MONTHS: [&str; 12] = [
    "JANUARY",
    "FEBRUARY",
    "MARCH",
    "APRIL",
    "MAY",
    "JUNE",
    "JULY",
    "AUGUST",
    "SEPTEMBER",
    "OCTOBER",
    "NOVEMBER",
    "DECEMBER",
];

/// A date-construction failure (the JDK message; the port has no
/// `DateTimeException` variant, so the class is
/// `IllegalArgumentException`).
fn dt_err(msg: String) -> NativeError {
    NativeError::new(ThrowClass::IllegalArgumentException, msg)
}

/// Validate `LocalDate/of` args (month first, like the JDK).
fn check_date(y: i64, m: i64, d: i64) -> Result<EmuLocalDate, NativeError> {
    if !(-999_999_999..=999_999_999).contains(&y) {
        return Err(dt_err(format!(
            "Invalid value for Year (valid values -999999999 - 999999999): {y}"
        )));
    }
    if !(1..=12).contains(&m) {
        return Err(dt_err(format!(
            "Invalid value for MonthOfYear (valid values 1 - 12): {m}"
        )));
    }
    if !(1..=days_in_month(y, m)).contains(&d) {
        return Err(dt_err(format!(
            "Invalid date '{} {}'",
            MONTHS[(m - 1) as usize],
            d
        )));
    }
    Ok(EmuLocalDate {
        y: y as i32,
        m: m as u8,
        d: d as u8,
    })
}

/// Validate `LocalDateTime/of` time args.
fn check_time(h: i64, mi: i64, s: i64) -> Result<EmuLocalTime, NativeError> {
    if !(0..=23).contains(&h) {
        return Err(dt_err(format!(
            "Invalid value for HourOfDay (valid values 0 - 23): {h}"
        )));
    }
    if !(0..=59).contains(&mi) {
        return Err(dt_err(format!(
            "Invalid value for MinuteOfHour (valid values 0 - 59): {mi}"
        )));
    }
    if !(0..=59).contains(&s) {
        return Err(dt_err(format!(
            "Invalid value for SecondOfMinute (valid values 0 - 59): {s}"
        )));
    }
    Ok(EmuLocalTime {
        h: h as u8,
        mi: mi as u8,
        s: s as u8,
    })
}

/// The ISO day-of-week number (1 = Monday through 7 = Sunday):
/// 1970-01-01 was a Thursday.
fn dow_of(d: EmuLocalDate) -> u8 {
    let z = days_since_epoch(d.y, d.m, d.d);
    (((z % 7 + 7 + 3) % 7) + 1) as u8
}

/// Uppercase day names (the `print` shape).
const DAYS: [&str; 7] = [
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
];

/// Unix seconds now (the clock error reads the epoch — a clock
/// before 1970 is out of the suite's reach).
fn unix_secs() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    i64::try_from(secs).unwrap_or(i64::MAX)
}

/// Split unix seconds into civil date plus time-of-day.
fn split_epoch(secs: i64) -> (EmuLocalDate, EmuLocalTime) {
    let days = secs.div_euclid(86400);
    let tod = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    // Clock-range days always land inside the `i32` year range.
    let date = EmuLocalDate { y: y as i32, m, d };
    let time = EmuLocalTime {
        h: (tod / 3600) as u8,
        mi: ((tod % 3600) / 60) as u8,
        s: (tod % 60) as u8,
    };
    (date, time)
}

/// One char inside a quoted run: a doubled quote is a literal
/// quote, a lone quote closes the run, anything else is literal.
/// Returns whether the run stays open.
fn push_quoted(c: char, chars: &mut Peekable<Chars<'_>>, elems: &mut Vec<PatElem>) -> bool {
    if c != '\'' {
        elems.push(PatElem::Lit(c));
        return true;
    }
    if chars.peek() == Some(&'\'') {
        chars.next();
        elems.push(PatElem::Lit('\''));
        true
    } else {
        false
    }
}

/// A padded pair (`M d H m s` — the caller maps width 1 to the
/// bare elem and width 2 to the padded one; wider runs fail there
/// with the JDK-shaped message).
fn padded_directive(c: char, padded: bool) -> Option<PatElem> {
    match (c, padded) {
        ('M', false) => Some(PatElem::Month),
        ('M', true) => Some(PatElem::MonthPad),
        ('d', false) => Some(PatElem::Day),
        ('d', true) => Some(PatElem::DayPad),
        ('H', false) => Some(PatElem::Hour),
        ('H', true) => Some(PatElem::HourPad),
        ('m', false) => Some(PatElem::Min),
        ('m', true) => Some(PatElem::MinPad),
        ('s', false) => Some(PatElem::Sec),
        ('s', true) => Some(PatElem::SecPad),
        _ => None,
    }
}

/// One letter run outside quotes (`y` at any width, the padded
/// pairs at width 1–2 — anything else fails like the JDK's eager
/// validation).
fn compile_directive(c: char, width: usize) -> Result<PatElem, NativeError> {
    if c == 'y' {
        return Ok(PatElem::Year(width));
    }
    match padded_directive(c, width == 2) {
        Some(elem) => {
            if width > 2 {
                return Err(dt_err(format!(
                    "Unsupported DateTimeFormatter pattern: {c} run of width {width}"
                )));
            }
            Ok(elem)
        }
        None => Err(dt_err(format!("Unknown pattern letter: {c}"))),
    }
}

/// Compile an `ofPattern` pattern (ASCII letters outside quotes
/// must be `yMdHms` runs — anything else fails here, like the
/// JDK's eager validation).
fn compile_pattern(pattern: &str) -> Result<Vec<PatElem>, NativeError> {
    let mut elems = Vec::new();
    let mut chars = pattern.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if quoted {
            quoted = push_quoted(c, &mut chars, &mut elems);
            continue;
        }
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                elems.push(PatElem::Lit('\''));
            } else {
                quoted = true;
            }
            continue;
        }
        if !c.is_ascii_alphabetic() {
            elems.push(PatElem::Lit(c));
            continue;
        }
        let mut width = 1;
        while chars.peek() == Some(&c) {
            chars.next();
            width += 1;
        }
        elems.push(compile_directive(c, width)?);
    }
    if quoted {
        return Err(dt_err("Pattern ends with unbalanced quotes".to_owned()));
    }
    Ok(elems)
}

/// Render the year run (`yy` the century-free pair, wider runs
/// zero-padded — negatives never pad, like the JDK).
fn render_year(out: &mut String, y: i32, w: usize) {
    let y = i64::from(y);
    if w == 2 {
        out.push_str(&format!("{:02}", y.rem_euclid(100)));
    } else if y < 0 {
        out.push_str(&y.to_string());
    } else {
        let digits = y.to_string();
        for _ in digits.len()..w {
            out.push('0');
        }
        out.push_str(&digits);
    }
}

/// Render a datetime through compiled runs.
fn render(dt: EmuLocalDateTime, elems: &[PatElem]) -> String {
    let mut out = String::new();
    for elem in elems {
        match elem {
            PatElem::Year(w) => render_year(&mut out, dt.date.y, *w),
            PatElem::Month => out.push_str(&dt.date.m.to_string()),
            PatElem::MonthPad => out.push_str(&format!("{:02}", dt.date.m)),
            PatElem::Day => out.push_str(&dt.date.d.to_string()),
            PatElem::DayPad => out.push_str(&format!("{:02}", dt.date.d)),
            PatElem::Hour => out.push_str(&dt.time.h.to_string()),
            PatElem::HourPad => out.push_str(&format!("{:02}", dt.time.h)),
            PatElem::Min => out.push_str(&dt.time.mi.to_string()),
            PatElem::MinPad => out.push_str(&format!("{:02}", dt.time.mi)),
            PatElem::Sec => out.push_str(&dt.time.s.to_string()),
            PatElem::SecPad => out.push_str(&format!("{:02}", dt.time.s)),
            PatElem::Lit(c) => out.push(*c),
        }
    }
    out
}

/// A null formatter/zone arg (the JDK NPEs).
fn null_arg(what: &str) -> NativeError {
    NativeError::new(ThrowClass::NullPointerException, format!("{what} is null"))
}

/// Dispatch over a `LocalDate`.
fn localdate_call(d: &EmuLocalDate, meth: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.LocalDate";
    match (meth, rest.len()) {
        ("getYear", 0) => Ok(Value::Int(i64::from(d.y))),
        ("getMonthValue", 0) => Ok(Value::Int(i64::from(d.m))),
        ("getDayOfMonth", 0) => Ok(Value::Int(i64::from(d.d))),
        ("getDayOfWeek", 0) => Ok(Value::Extension(Arc::new(EmuDayOfWeek(dow_of(*d))))),
        ("plusDays", 1) => {
            let n = crate::classemu::long_arg(&rest[0])?;
            let days = days_since_epoch(d.y, d.m, d.d).saturating_add(n);
            let (y, m, dd) = civil_from_days(days);
            check_date(y, m.into(), dd.into()).map(date_value)
        }
        _ => Err(crate::types::no_method(meth, rest.len(), CLASS)),
    }
}

/// Dispatch over a `DayOfWeek`.
fn dow_call(w: u8, meth: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.DayOfWeek";
    match (meth, rest.len()) {
        ("getValue", 0) => Ok(Value::Int(i64::from(w))),
        _ => Err(crate::types::no_method(meth, rest.len(), CLASS)),
    }
}

/// Dispatch over a `LocalDateTime`.
fn datetime_call(dt: &EmuLocalDateTime, meth: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.LocalDateTime";
    match (meth, rest.len()) {
        ("format", 1) => {
            if matches!(rest[0], Value::Nil) {
                return Err(null_arg("formatter"));
            }
            let Some(f) = as_formatter(&rest[0]) else {
                return Err(crate::core::cce_err(
                    &rest[0],
                    "java.time.format.DateTimeFormatter",
                ));
            };
            Ok(Value::Str(Arc::from(render(*dt, &f.elems).as_str())))
        }
        ("atZone", 1) => {
            let zone = zone_arg(&rest[0])?;
            Ok(Value::Extension(Arc::new(EmuZonedDateTime {
                dt: *dt,
                offset_secs: zone.secs,
            })))
        }
        _ => Err(crate::types::no_method(meth, rest.len(), CLASS)),
    }
}

/// Dispatch over an `Instant`.
fn instant_call(i: &EmuInstant, meth: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.Instant";
    match (meth, rest.len()) {
        ("atZone", 1) => {
            let zone = zone_arg(&rest[0])?;
            let (date, time) = split_epoch(i.secs + i64::from(zone.secs));
            Ok(Value::Extension(Arc::new(EmuZonedDateTime {
                dt: EmuLocalDateTime { date, time },
                offset_secs: zone.secs,
            })))
        }
        _ => Err(crate::types::no_method(meth, rest.len(), CLASS)),
    }
}

/// Dispatch over a `ZonedDateTime`.
fn zoned_call(z: &EmuZonedDateTime, meth: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.ZonedDateTime";
    match (meth, rest.len()) {
        ("toLocalDate", 0) => Ok(date_value(z.dt.date)),
        ("toEpochSecond", 0) => {
            let days = days_since_epoch(z.dt.date.y, z.dt.date.m, z.dt.date.d);
            let tod = i64::from(z.dt.time.h) * 3600
                + i64::from(z.dt.time.mi) * 60
                + i64::from(z.dt.time.s);
            Ok(Value::Int(days * 86400 + tod - i64::from(z.offset_secs)))
        }
        _ => Err(crate::types::no_method(meth, rest.len(), CLASS)),
    }
}

/// A `ZoneId` arg (only `ZoneOffset` inhabitants exist; nil is the
/// JDK NPE).
fn zone_arg(v: &Value) -> Result<EmuZoneOffset, NativeError> {
    if matches!(v, Value::Nil) {
        return Err(null_arg("zone"));
    }
    as_zone(v).ok_or_else(|| crate::core::cce_err(v, "java.time.ZoneId"))
}

/// `LocalDate` static arities (`of` the 3-arg, `now` the zone).
pub(crate) fn static_arities_localdate(member: &str) -> Option<&'static [usize]> {
    match member {
        "of" => Some(&[3]),
        "now" => Some(&[1]),
        _ => None,
    }
}

/// `LocalDateTime` static arities (`of` the 5/6-arg, `now` bare).
pub(crate) fn static_arities_datetime(member: &str) -> Option<&'static [usize]> {
    match member {
        "of" => Some(&[5, 6]),
        "now" => Some(&[0]),
        _ => None,
    }
}

/// `Instant` static arities (`ofEpochSecond` the 1-arg).
pub(crate) fn static_arities_instant(member: &str) -> Option<&'static [usize]> {
    match member {
        "ofEpochSecond" => Some(&[1]),
        _ => None,
    }
}

/// `DateTimeFormatter` static arities (`ofPattern` the 1-arg).
pub(crate) fn static_arities_formatter(member: &str) -> Option<&'static [usize]> {
    match member {
        "ofPattern" => Some(&[1]),
        _ => None,
    }
}

/// `LocalDate/of` and `LocalDate/now` (the zone reads the clock as
/// UTC — only UTC inhabitants exist).
pub(crate) fn static_call_localdate(member: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.LocalDate";
    match member {
        "of" => {
            if rest.len() != 3 {
                return Err(crate::types::no_method(member, rest.len(), CLASS));
            }
            let y = crate::classemu::long_arg(&rest[0])?;
            let m = crate::classemu::long_arg(&rest[1])?;
            let d = crate::classemu::long_arg(&rest[2])?;
            check_date(y, m, d).map(date_value)
        }
        "now" => {
            if rest.len() != 1 {
                return Err(crate::types::no_method(member, rest.len(), CLASS));
            }
            let zone = zone_arg(&rest[0])?;
            let (date, _) = split_epoch(unix_secs() + i64::from(zone.secs));
            Ok(date_value(date))
        }
        _ => Err(crate::types::no_method(member, rest.len(), CLASS)),
    }
}

/// `LocalDateTime/of` (the 5/6-arg — seconds default to zero).
fn static_datetime_of(member: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.LocalDateTime";
    if rest.len() != 5 && rest.len() != 6 {
        return Err(crate::types::no_method(member, rest.len(), CLASS));
    }
    let y = crate::classemu::long_arg(&rest[0])?;
    let m = crate::classemu::long_arg(&rest[1])?;
    let d = crate::classemu::long_arg(&rest[2])?;
    let h = crate::classemu::long_arg(&rest[3])?;
    let mi = crate::classemu::long_arg(&rest[4])?;
    let s = if rest.len() == 6 {
        crate::classemu::long_arg(&rest[5])?
    } else {
        0
    };
    let date = check_date(y, m, d)?;
    let time = check_time(h, mi, s)?;
    Ok(datetime_value(EmuLocalDateTime { date, time }))
}

/// `LocalDateTime/of` and `LocalDateTime/now` (bare `now` reads
/// the clock as UTC — the suite asserts shape only).
pub(crate) fn static_call_datetime(member: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.LocalDateTime";
    match member {
        "of" => static_datetime_of(member, rest),
        "now" => {
            if !rest.is_empty() {
                return Err(crate::types::no_method(member, rest.len(), CLASS));
            }
            let (date, time) = split_epoch(unix_secs());
            Ok(datetime_value(EmuLocalDateTime { date, time }))
        }
        _ => Err(crate::types::no_method(member, rest.len(), CLASS)),
    }
}

/// `Instant/ofEpochSecond` (the 1-arg).
pub(crate) fn static_call_instant(member: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.Instant";
    match member {
        "ofEpochSecond" => {
            if rest.len() != 1 {
                return Err(crate::types::no_method(member, rest.len(), CLASS));
            }
            let secs = crate::classemu::long_arg(&rest[0])?;
            Ok(Value::Extension(Arc::new(EmuInstant { secs })))
        }
        _ => Err(crate::types::no_method(member, rest.len(), CLASS)),
    }
}

/// `DateTimeFormatter/ofPattern` (nil is the JDK NPE; anything
/// else non-string is the overload miss).
pub(crate) fn static_call_formatter(member: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.time.format.DateTimeFormatter";
    match member {
        "ofPattern" => {
            if rest.len() != 1 {
                return Err(crate::types::no_method(member, rest.len(), CLASS));
            }
            if matches!(rest[0], Value::Nil) {
                return Err(null_arg("pattern"));
            }
            let Value::Str(pattern) = &rest[0] else {
                return Err(crate::types::no_method(member, rest.len(), CLASS));
            };
            let elems = compile_pattern(pattern)?;
            Ok(Value::Extension(Arc::new(EmuDateTimeFormatter {
                pattern: Arc::from(pattern.as_ref()),
                elems,
            })))
        }
        _ => Err(crate::types::no_method(member, rest.len(), CLASS)),
    }
}

/// The `ZoneOffset/UTC` field (fresh per read, like every other
/// field — `=` holds, `identical?` does not).
pub(crate) fn static_field_zoneoffset(field: &str) -> Option<Value> {
    match field {
        "UTC" => Some(Value::Extension(Arc::new(EmuZoneOffset { secs: 0 }))),
        _ => None,
    }
}

impl ExtensionValue for EmuLocalDate {
    fn class_name(&self) -> String {
        "java.time.LocalDate".to_owned()
    }

    fn print(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuLocalDate>()
            .is_some_and(|o| o == self)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u8(0xD0);
        state.write_i32(self.y);
        state.write_u8(self.m);
        state.write_u8(self.d);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ExtensionValue for EmuLocalDateTime {
    fn class_name(&self) -> String {
        "java.time.LocalDateTime".to_owned()
    }

    fn print(&self) -> String {
        self.iso()
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuLocalDateTime>()
            .is_some_and(|o| o == self)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u8(0xD1);
        state.write_i32(self.date.y);
        state.write_u8(self.date.m);
        state.write_u8(self.date.d);
        state.write_u8(self.time.h);
        state.write_u8(self.time.mi);
        state.write_u8(self.time.s);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ExtensionValue for EmuInstant {
    fn class_name(&self) -> String {
        "java.time.Instant".to_owned()
    }

    fn print(&self) -> String {
        let (date, time) = split_epoch(self.secs);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            date.y, date.m, date.d, time.h, time.mi, time.s
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuInstant>()
            .is_some_and(|o| o == self)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u8(0xD2);
        state.write_i64(self.secs);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ExtensionValue for EmuZoneOffset {
    fn class_name(&self) -> String {
        "java.time.ZoneOffset".to_owned()
    }

    fn print(&self) -> String {
        // Only UTC inhabitants exist (sole constructor above).
        debug_assert_eq!(self.secs, 0);
        "Z".to_owned()
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuZoneOffset>()
            .is_some_and(|o| o == self)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u8(0xD3);
        state.write_i32(self.secs);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ExtensionValue for EmuZonedDateTime {
    fn class_name(&self) -> String {
        "java.time.ZonedDateTime".to_owned()
    }

    fn print(&self) -> String {
        let zone = EmuZoneOffset {
            secs: self.offset_secs,
        };
        // Only UTC inhabitants exist, so the zone id is `UTC`.
        format!("{}{}[UTC]", self.dt.iso(), zone.print())
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuZonedDateTime>()
            .is_some_and(|o| o == self)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u8(0xD4);
        state.write_i32(self.dt.date.y);
        state.write_u8(self.dt.date.m);
        state.write_u8(self.dt.date.d);
        state.write_u8(self.dt.time.h);
        state.write_u8(self.dt.time.mi);
        state.write_u8(self.dt.time.s);
        state.write_i32(self.offset_secs);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl EmuLocalDateTime {
    /// The JDK `toString` (seconds vanish when zero).
    fn iso(&self) -> String {
        let base = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}",
            self.date.y, self.date.m, self.date.d, self.time.h, self.time.mi
        );
        if self.time.s == 0 {
            base
        } else {
            format!("{base}:{:02}", self.time.s)
        }
    }
}

impl ExtensionValue for EmuDayOfWeek {
    fn class_name(&self) -> String {
        "java.time.DayOfWeek".to_owned()
    }

    fn print(&self) -> String {
        DAYS[(self.0 - 1) as usize].to_owned()
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuDayOfWeek>()
            .is_some_and(|o| o == self)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u8(0xD5);
        state.write_u8(self.0);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ExtensionValue for EmuDateTimeFormatter {
    fn class_name(&self) -> String {
        "java.time.format.DateTimeFormatter".to_owned()
    }

    /// The JDK `toString` (the pattern).
    fn print(&self) -> String {
        self.pattern.to_string()
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuDateTimeFormatter>()
            .is_some_and(|o| o == self)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        // Runs derive from the pattern, so the pattern alone hashes.
        state.write_u8(0xD6);
        state.write(self.pattern.as_bytes());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
