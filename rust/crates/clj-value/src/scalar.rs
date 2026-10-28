// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S9 scalar kinds: UUID, Instant, URI (issue #52).
//!
//! [`Uuid`] replicates `java.util.UUID` (`fromString` both paths,
//! `toString`, `hashCode`, `compareTo`); [`Instant`] is millis
//! since the epoch with `clojure.instant` construction (lenient
//! `GregorianCalendar` with the default 1582 cutover) and printing
//! (`SimpleDateFormat` / `Date.toString` emulations, UTC);
//! [`Uri`] ports the JDK single-arg parser plus `equals` /
//! `hashCode` / `compareTo`. Oracle:
//! `docs/zero-oos-s9-scalarkinds/probes/*.out`.

use std::cmp::Ordering;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use crate::datareaders::{TagError, validate_inst};

/// A UUID: the 128 bits as most/least significant words (S9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Uuid {
    /// The most significant 64 bits.
    pub hi: u64,
    /// The least significant 64 bits.
    pub lo: u64,
}

impl Uuid {
    /// Build from raw words (no masking — construction paths mask).
    #[must_use]
    pub fn from_bits(hi: u64, lo: u64) -> Self {
        Self { hi, lo }
    }

    /// Stamp two random words as a version-4 UUID (the
    /// `randomUUID` bit-twiddle; the draws come from the caller's
    /// RNG — `random-uuid` uses the `rng.rs` source).
    #[must_use]
    pub fn from_random(hi: u64, lo: u64) -> Self {
        Self {
            hi: (hi & 0xffff_ffff_ffff_0fff) | 0x4000,
            lo: (lo & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000,
        }
    }

    /// `toString`: lowercase canonical with fixed dashes.
    #[must_use]
    pub fn to_canonical(&self) -> String {
        let h = self.hi;
        let l = self.lo;
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (h >> 32) as u32,
            ((h >> 16) & 0xffff) as u16,
            (h & 0xffff) as u16,
            ((l >> 48) & 0xffff) as u16,
            l & 0xffff_ffff_ffff,
        )
    }

    /// `UUID.hashCode`: the 128 bits folded to `i32`.
    #[must_use]
    pub fn hash_code(&self) -> i32 {
        let hilo = self.hi ^ self.lo;
        ((hilo >> 32) as u32 ^ (hilo & 0xffff_ffff) as u32) as i32
    }

    /// `UUID.compareTo`: signed `hi`, then signed `lo`.
    #[must_use]
    pub fn cmp_uuid(&self, other: &Self) -> Ordering {
        (self.hi as i64)
            .cmp(&(other.hi as i64))
            .then_with(|| (self.lo as i64).cmp(&(other.lo as i64)))
    }
}

/// `UUID.fromString`: the length-36 fast path plus the lenient
/// slow path (five dash-separated groups, each a signed hex
/// `long`, masked to 32/16/16/16/48 bits). Errors mirror the
/// JDK texts (`TagError`, like the S6 validators).
impl FromStr for Uuid {
    type Err = TagError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let units: Vec<u16> = s.encode_utf16().collect();
        if let Some(u) = uuid_fast(&units) {
            return Ok(u);
        }
        uuid_slow(s, &units)
    }
}

/// One hex nibble's value (`None` past Latin-1 or off the table).
fn nibble(unit: u16) -> Option<u64> {
    if unit > 0xff {
        return None;
    }
    match unit as u8 {
        b'0'..=b'9' => Some(u64::from(unit - u16::from(b'0'))),
        b'a'..=b'f' => Some(u64::from(unit - u16::from(b'a')) + 10),
        b'A'..=b'F' => Some(u64::from(unit - u16::from(b'A')) + 10),
        _ => None,
    }
}

/// Four nibbles at a UTF-16 offset (`None` on any bad unit — the
/// JDK's `parse4Nibbles` returning -1).
fn parse4(units: &[u16], pos: usize) -> Option<u64> {
    let a = nibble(*units.get(pos)?)?;
    let b = nibble(*units.get(pos + 1)?)?;
    let c = nibble(*units.get(pos + 2)?)?;
    let d = nibble(*units.get(pos + 3)?)?;
    Some(a << 12 | b << 8 | c << 4 | d)
}

/// The length-36 fast path (`None` falls through to the slow path,
/// even for fixed-dash shapes with bad nibbles).
fn uuid_fast(units: &[u16]) -> Option<Uuid> {
    if units.len() != 36 {
        return None;
    }
    if units[8] != u16::from(b'-')
        || units[13] != u16::from(b'-')
        || units[18] != u16::from(b'-')
        || units[23] != u16::from(b'-')
    {
        return None;
    }
    let msb = parse4(units, 0)? << 48
        | parse4(units, 4)? << 32
        | parse4(units, 9)? << 16
        | parse4(units, 14)?;
    let lsb = parse4(units, 19)? << 48
        | parse4(units, 24)? << 32
        | parse4(units, 28)? << 16
        | parse4(units, 32)?;
    Some(Uuid::from_bits(msb, lsb))
}

/// Dash slots of the slow path (exactly four dashes or the shape
/// error — the JDK's `dash4 < 0 || dash5 >= 0` check).
fn uuid_dashes(units: &[u16]) -> Option<[usize; 4]> {
    let mut dashes = [0usize; 4];
    let mut found = 0usize;
    for (i, u) in units.iter().enumerate() {
        if *u == u16::from(b'-') {
            if found >= 4 {
                return None;
            }
            dashes[found] = i;
            found += 1;
        }
    }
    if found == 4 { Some(dashes) } else { None }
}

/// `Long.parseLong` over a UTF-16 group: an optional ASCII sign,
/// then hex digits with overflow detection. Astral input arrives
/// as surrogate halves, which fail as non-hex at their UTF-16
/// index — exactly like `parseLong`.
fn parse_group(units: &[u16]) -> Result<i64, GroupError> {
    let chars: Vec<char> = units
        .iter()
        .map(|u| char::from_u32(u32::from(*u)).unwrap_or('\u{FFFD}'))
        .collect();
    parse_group_chars(&chars)
}

/// The error kind of a group parse (split for the budget).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupError {
    /// No digits (the empty range, or a lone sign).
    Empty,
    /// A bad digit or an overflow at the relative index.
    BadIndex(usize),
}

/// Parse one group over chars (the sign consumes an index, like
/// `parseLong` over the range).
fn parse_group_chars(chars: &[char]) -> Result<i64, GroupError> {
    let digits = skip_group_sign(chars);
    if digits.is_empty() {
        return Err(GroupError::Empty);
    }
    let mut value: u64 = 0;
    for (i, c) in digits.iter().enumerate() {
        let Some(d) = c.to_digit(16) else {
            return Err(GroupError::BadIndex(sign_len(chars) + i));
        };
        value = match value
            .checked_mul(16)
            .and_then(|v| v.checked_add(u64::from(d)))
        {
            Some(v) => v,
            None => return Err(GroupError::BadIndex(sign_len(chars) + i)),
        };
        if !is_negative(chars) && value > i64::MAX as u64 {
            return Err(GroupError::BadIndex(sign_len(chars) + i));
        }
        if is_negative(chars) && value > i64::MAX as u64 + 1 {
            return Err(GroupError::BadIndex(sign_len(chars) + i));
        }
    }
    Ok(apply_group_sign(chars, value))
}

/// The signed group value (split for the budget): `i64::MIN` is
/// valid when negative (the `parseLong` asymmetry).
fn apply_group_sign(chars: &[char], value: u64) -> i64 {
    if is_negative(chars) {
        (value as i64).wrapping_neg()
    } else {
        value as i64
    }
}

/// A leading ASCII sign (split for the budget).
fn skip_group_sign(chars: &[char]) -> &[char] {
    match chars.first() {
        Some(&c) if c == '+' || c == '-' => &chars[1..],
        _ => chars,
    }
}

/// Whether the group is negative (split for the budget).
fn is_negative(chars: &[char]) -> bool {
    matches!(chars.first(), Some('-'))
}

/// The sign width in chars (split for the budget).
fn sign_len(chars: &[char]) -> usize {
    usize::from(matches!(chars.first(), Some('+' | '-')))
}

/// A `TagError` with the JDK simple class name (the struct's
/// fields are public; these mirror S6's private conveniences).
fn tag_iae(message: String) -> TagError {
    TagError {
        class: "IllegalArgumentException",
        message,
    }
}

/// A `TagError` with the JDK simple class name (see [`tag_iae`]).
fn tag_nfe(message: String) -> TagError {
    TagError {
        class: "NumberFormatException",
        message,
    }
}

/// The lenient slow path (the JDK's `fromString1`).
fn uuid_slow(s: &str, units: &[u16]) -> Result<Uuid, TagError> {
    if units.len() > 36 {
        return Err(tag_iae("UUID string too large".to_owned()));
    }
    let Some(d) = uuid_dashes(units) else {
        return Err(tag_iae(format!("Invalid UUID string: {s}")));
    };
    let bounds = [
        (0, d[0]),
        (d[0] + 1, d[1]),
        (d[1] + 1, d[2]),
        (d[2] + 1, d[3]),
        (d[3] + 1, units.len()),
    ];
    let mut groups = [0i64; 5];
    for (i, (from, to)) in bounds.iter().enumerate() {
        let part: String = units[*from..*to]
            .iter()
            .map(|u| char::from_u32(u32::from(*u)).unwrap_or('\u{FFFD}'))
            .collect();
        let part_units = &units[*from..*to];
        groups[i] = parse_group(part_units).map_err(|e| group_error(&part, e))?;
    }
    let hi = ((groups[0] as u64) & 0xffff_ffff) << 32
        | (((groups[1] as u64) & 0xffff) << 16)
        | ((groups[2] as u64) & 0xffff);
    let lo = (((groups[3] as u64) & 0xffff) << 48) | ((groups[4] as u64) & 0xffff_ffff_ffff);
    Ok(Uuid::from_bits(hi, lo))
}

/// A group failure as the JDK's NFE text (split for the budget).
fn group_error(part: &str, err: GroupError) -> TagError {
    match err {
        GroupError::Empty => tag_nfe("For input string: \"\" under radix 16".to_owned()),
        GroupError::BadIndex(i) => tag_nfe(format!("Error at index {i} in: \"{part}\"")),
    }
}

/// An instant: millis since the epoch, UTC (S9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Instant {
    /// Millis since 1970-01-01T00:00:00Z.
    pub millis: i64,
}

impl Instant {
    /// Build from millis.
    #[must_use]
    pub fn from_millis(millis: i64) -> Self {
        Self { millis }
    }

    /// `read-instant-date` over an already-validated payload: split
    /// and compute millis through the lenient cutover calendar
    /// (the payload zone; a missing zone is GMT).
    fn from_validated(s: &str) -> Self {
        let parts = parse_validated(s).expect("S6-validated payloads always split");
        Self::from_millis(parts.millis())
    }

    /// `Date.hashCode`: the millis folded to `i32`.
    #[must_use]
    pub fn hash_code(&self) -> i32 {
        let t = self.millis as u64;
        ((t >> 32) as u32 ^ (t & 0xffff_ffff) as u32) as i32
    }

    /// `Date.compareTo`: millis order.
    #[must_use]
    pub fn cmp_instant(&self, other: &Self) -> Ordering {
        self.millis.cmp(&other.millis)
    }

    /// The `#inst` payload: UTC `yyyy-MM-dd'T'HH:mm:ss.SSS` (the
    /// `SimpleDateFormat` emulation — cutover-aware fields, the
    /// literal `-00:00` suffix rides in `print.rs`).
    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        let f = fields_from_millis(self.millis);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            f.print_year(),
            f.month,
            f.day,
            f.hour,
            f.minute,
            f.second,
            f.milli,
        )
    }

    /// `Date.toString` at UTC (the `str`/`%s` rendering):
    /// `EEE MMM dd HH:mm:ss zzz yyyy` with US names.
    #[must_use]
    pub fn to_date_string(&self) -> String {
        let f = fields_from_millis(self.millis);
        format!(
            "{} {} {:02} {:02}:{:02}:{:02} UTC {}",
            f.weekday_abbr(),
            f.month_abbr(),
            f.day,
            f.hour,
            f.minute,
            f.second,
            f.year_label(),
        )
    }
}

/// `read-instant-date`: validate through the S6 grammar, then
/// compute millis through the lenient cutover calendar (the
/// payload zone; a missing zone is GMT).
impl FromStr for Instant {
    type Err = TagError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate_inst(s)?;
        Ok(Self::from_validated(s))
    }
}

/// One validated `#inst` payload's components (only present parts;
/// the S6 validator guarantees the grammar).
#[derive(Clone, Copy, Debug, Default)]
struct InstParts {
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
    offset_minutes: i32,
}

impl InstParts {
    /// Millis through the lenient cutover calendar (time-of-day as
    /// total seconds, so `:59:60` rolls; minus the zone offset).
    fn millis(&self) -> i64 {
        let days = days_cutover(self.year, self.month, self.day);
        let tod =
            i64::from(self.hour) * 3_600 + i64::from(self.minute) * 60 + i64::from(self.second);
        days * 86_400_000 + tod * 1_000 + i64::from(self.millis)
            - i64::from(self.offset_minutes) * 60_000
    }
}

/// Split a validated payload into components. The grammar walk
/// mirrors S6's `parse_instant` deliberately (that module is S6's;
/// this slice reuses only its public `validate_inst`): ASCII
/// digits, fixed positions, `T`/zone suffixes.
fn parse_validated(s: &str) -> Option<InstParts> {
    let bytes = s.as_bytes();
    let mut parts = InstParts {
        month: 1,
        day: 1,
        ..InstParts::default()
    };
    parts.year = digits_at(bytes, 0, 4)?;
    let mut end = 4;
    end = parse_date_prefix(bytes, end, &mut parts)?;
    end = parse_validated_time(bytes, end, &mut parts)?;
    parts.offset_minutes = parse_validated_zone(bytes, end)?;
    Some(parts)
}

/// Read `n` ASCII digits at `pos` (`None` past the end or on a
/// non-digit).
fn digits_at(bytes: &[u8], pos: usize, n: usize) -> Option<u32> {
    let mut value = 0u32;
    for i in 0..n {
        let c = *bytes.get(pos + i)?;
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(c - b'0');
    }
    Some(value)
}

/// The optional `-MM` / `-DD` prefixes (split for the budget).
fn parse_date_prefix(bytes: &[u8], end: usize, parts: &mut InstParts) -> Option<usize> {
    let mut end = end;
    if bytes.get(end) == Some(&b'-') && bytes.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
        parts.month = digits_at(bytes, end + 1, 2)?;
        end += 3;
        if bytes.get(end) == Some(&b'-') && bytes.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
            parts.day = digits_at(bytes, end + 1, 2)?;
            end += 3;
        }
    }
    Some(end)
}

/// The optional `THH:MM(:SS(.frac)?)?` part (split for the budget).
fn parse_validated_time(bytes: &[u8], end: usize, parts: &mut InstParts) -> Option<usize> {
    if bytes.get(end) != Some(&b'T') {
        return Some(end);
    }
    let mut end = end + 1;
    parts.hour = digits_at(bytes, end, 2)?;
    if bytes.get(end + 2) != Some(&b':') {
        return None;
    }
    end += 3;
    parts.minute = digits_at(bytes, end, 2)?;
    end += 2;
    if bytes.get(end) != Some(&b':') {
        return Some(end);
    }
    end += 1;
    parts.second = digits_at(bytes, end, 2)?;
    end += 2;
    if bytes.get(end) != Some(&b'.') {
        return Some(end);
    }
    end = parse_frac(bytes, end + 1, parts);
    Some(end)
}

/// The fraction digits, truncated to millis (split for the budget).
fn parse_frac(bytes: &[u8], mut end: usize, parts: &mut InstParts) -> usize {
    let mut factor = 100u32;
    while let Some(c) = bytes.get(end) {
        if !c.is_ascii_digit() || factor == 0 {
            break;
        }
        parts.millis += u32::from(c - b'0') * factor;
        factor /= 10;
        end += 1;
    }
    while bytes.get(end).is_some_and(|c| c.is_ascii_digit()) {
        end += 1;
    }
    end
}

/// The optional `Z` / `[+-]HH:MM` zone as offset minutes (split for
/// the budget; a missing zone is GMT).
fn parse_validated_zone(bytes: &[u8], end: usize) -> Option<i32> {
    if bytes.get(end) == Some(&b'Z') {
        return Some(0);
    }
    let sign = match bytes.get(end) {
        Some(b'+') => 1,
        Some(b'-') => -1,
        None => return Some(0),
        _ => return None,
    };
    let hours = digits_at(bytes, end + 1, 2)?;
    if bytes.get(end + 3) != Some(&b':') {
        return None;
    }
    let minutes = digits_at(bytes, end + 4, 2)?;
    Some(sign * (hours as i32 * 60 + minutes as i32))
}

/// Days since the epoch for a field date under the cutover: Julian
/// before 1582-10-15 (gap days included — the Julian count is
/// continuous, so the lenient +10 roll emerges), proleptic
/// Gregorian on/after.
fn days_cutover(year: u32, month: u32, day: u32) -> i64 {
    if (year, month, day) < (1582, 10, 15) {
        days_julian(year, month, day)
    } else {
        days_proleptic(year, month, day)
    }
}

/// Civil date → days since the epoch (Howard Hinnant, proleptic
/// Gregorian; mirrors the `fmt.rs` helper, which lives in another
/// slice's file).
fn days_proleptic(year: u32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u64;
    let mp = (u64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + u64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Civil date → days since the epoch under Julian rules
/// (March-based 4-year eras; the anchor pins Julian(1582-10-04)
/// to -141428 — probe `e-cut-before` — and Julian(0001-01-01)
/// then lands on -719164, probe `i-ms-year1`).
fn days_julian(year: u32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era = y.div_euclid(4);
    let yoe = y.rem_euclid(4);
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + doy;
    era * 1_461 + doe - 719_470
}

/// Days since the epoch → civil date (Hinnant, proleptic
/// Gregorian; mirrors the `fmt.rs` helper).
fn civil_proleptic(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    (y, m, d)
}

/// Days since the epoch → civil date under Julian rules (the
/// 4-year-era inverse of [`days_julian`]; the leap day is
/// `doe == 1460`).
fn civil_julian(days: i64) -> (i32, u32, u32) {
    let z = days + 719_470;
    let era = z.div_euclid(1_461);
    let doe = (z - era * 1_461) as u64;
    let yoe = (doe / 365).min(3);
    let y = yoe as i64 + era * 4;
    let doy = doe - yoe * 365;
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    (y, m, d)
}

/// Millis of 1582-10-15T00:00:00Z: the cutover instant (fields at
/// or after it render proleptic Gregorian, before it Julian).
const CUTOVER_MILLIS: i64 = -12_219_292_800_000;

/// One instant's rendered fields (UTC, cutover-aware).
struct DateFields {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    milli: u32,
    weekday: u32,
}

impl DateFields {
    /// The `yyyy` label: era-shifted (`1 - year`) at/below year 0
    /// (probes `e-cut-pr-year0`, `e-str-year0`).
    fn print_year(&self) -> i32 {
        if self.year <= 0 {
            1 - self.year
        } else {
            self.year
        }
    }

    /// The `Date.toString` year label (unpadded, era-shifted).
    fn year_label(&self) -> String {
        self.print_year().to_string()
    }

    /// The US weekday abbreviation (Sunday-first).
    fn weekday_abbr(&self) -> &'static str {
        const ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        ABBR[self.weekday as usize]
    }

    /// The US month abbreviation.
    fn month_abbr(&self) -> &'static str {
        const ABBR: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        ABBR[(self.month - 1) as usize]
    }
}

/// A URI: the validated original string plus raw (never decoded)
/// parts, with the opaque flag and a construction id for the
/// `#object` hex (S9). Parts stay raw because `equals` /
/// `hashCode` / `compareTo` all run on raw forms (the JDK getters
/// decode on the fly; the port exposes no getters).
#[derive(Clone, Debug)]
pub struct Uri {
    /// The original string (`toString` echoes it verbatim).
    text: Arc<str>,
    scheme: Option<Arc<str>>,
    authority: Option<Arc<str>>,
    user_info: Option<Arc<str>>,
    host: Option<Arc<str>>,
    port: i32,
    path: Option<Arc<str>>,
    query: Option<Arc<str>>,
    fragment: Option<Arc<str>>,
    opaque: bool,
    ssp: Option<Arc<str>>,
    id: u64,
}

/// The construction-id supply (the `DELAY_ID` precedent).
static URI_ID: AtomicU64 = AtomicU64::new(1);

impl Uri {
    /// Parse one URI reference (the JDK single-arg constructor).
    pub fn parse(s: &str) -> Result<Self, UriError> {
        let parser = UriParser::new(s);
        parser.parse()
    }

    /// The original string.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The construction id (the `#object` hex).
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// `Hash` for map/set keys (`Eq`-consistent with [`Uri::uri_eq`]):
    /// the opaque flag plus each compared component in normal form
    /// (scheme/host fully lowercased, `%`-hex lowered elsewhere),
    /// length-prefixed and `None`-discriminated. Server URIs hash
    /// the `(userInfo, host, port)` triple — never the raw
    /// authority (`"h:"` and `"h"` are equal with undefined ports);
    /// registry URIs hash the authority.
    pub fn hash_content<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        self.opaque.hash(state);
        hash_part(state, self.scheme.as_deref(), true);
        hash_part(state, self.fragment.as_deref(), false);
        if self.opaque {
            hash_part(state, self.ssp.as_deref(), false);
        } else {
            hash_part(state, self.path.as_deref(), false);
            hash_part(state, self.query.as_deref(), false);
            if self.host.is_some() {
                hash_part(state, self.user_info.as_deref(), false);
                hash_part(state, self.host.as_deref(), true);
                state.write_i32(self.port);
            } else {
                hash_part(state, self.authority.as_deref(), false);
            }
        }
    }

    /// `URI.equals` (content only — the id never participates).
    #[must_use]
    pub fn uri_eq(&self, other: &Self) -> bool {
        if self.opaque != other.opaque {
            return false;
        }
        if !equal_ignoring_case(self.scheme.as_deref(), other.scheme.as_deref()) {
            return false;
        }
        if !uri_equal(self.fragment.as_deref(), other.fragment.as_deref()) {
            return false;
        }
        if self.opaque {
            return uri_equal(self.ssp.as_deref(), other.ssp.as_deref());
        }
        if !uri_equal(self.path.as_deref(), other.path.as_deref()) {
            return false;
        }
        if !uri_equal(self.query.as_deref(), other.query.as_deref()) {
            return false;
        }
        self.authority_eq(other)
    }

    /// The authority leg of [`Uri::uri_eq`] (split for the budget):
    /// server authorities compare the `(userInfo, host, port)`
    /// triple; anything else compares the raw authority normalized
    /// (`Some` vs `None` always differs).
    fn authority_eq(&self, other: &Self) -> bool {
        if self.authority == other.authority {
            return true;
        }
        if self.host.is_some() {
            return uri_equal(self.user_info.as_deref(), other.user_info.as_deref())
                && equal_ignoring_case(self.host.as_deref(), other.host.as_deref())
                && self.port == other.port;
        }
        uri_equal(self.authority.as_deref(), other.authority.as_deref())
    }

    /// `URI.hashCode`.
    #[must_use]
    pub fn hash_code(&self) -> i32 {
        let mut h = hash_ignoring_case(0, self.scheme.as_deref());
        h = uri_hash(h, self.fragment.as_deref());
        if self.opaque {
            h = uri_hash(h, self.ssp.as_deref());
        } else {
            h = uri_hash(h, self.path.as_deref());
            h = uri_hash(h, self.query.as_deref());
            if self.host.is_some() {
                h = uri_hash(h, self.user_info.as_deref());
                h = hash_ignoring_case(h, self.host.as_deref());
                h = h.wrapping_add(1949i32.wrapping_mul(self.port));
            } else {
                h = uri_hash(h, self.authority.as_deref());
            }
        }
        h
    }

    /// `URI.compareTo` (the raw `int`, like the JDK; the port's
    /// `compare` var normalizes the sign).
    #[must_use]
    pub fn compare_uri(&self, other: &Self) -> i32 {
        let c = compare_ignoring_case(self.scheme.as_deref(), other.scheme.as_deref());
        if c != 0 {
            return c;
        }
        if self.opaque || other.opaque {
            return self.compare_opaque_flag(other);
        }
        let c = self.compare_authority(other);
        if c != 0 {
            return c;
        }
        let c = uri_compare(self.path.as_deref(), other.path.as_deref());
        if c != 0 {
            return c;
        }
        let c = uri_compare(self.query.as_deref(), other.query.as_deref());
        if c != 0 {
            return c;
        }
        uri_compare(self.fragment.as_deref(), other.fragment.as_deref())
    }

    /// The opaque tail of `compareTo`: an opaque URI sorts after a
    /// hierarchical one; two opaque URIs compare scheme-specific
    /// part, then fragment.
    fn compare_opaque_flag(&self, other: &Self) -> i32 {
        if self.opaque {
            if other.opaque {
                let c = uri_compare(self.ssp.as_deref(), other.ssp.as_deref());
                if c != 0 {
                    return c;
                }
                return uri_compare(self.fragment.as_deref(), other.fragment.as_deref());
            }
            return 1;
        }
        -1
    }

    /// The authority leg of `compareTo`: user info, host
    /// (case-blind), then port when both sides parsed a server
    /// authority; the raw authority otherwise.
    fn compare_authority(&self, other: &Self) -> i32 {
        if self.host.is_some() && other.host.is_some() {
            let c = uri_compare(self.user_info.as_deref(), other.user_info.as_deref());
            if c != 0 {
                return c;
            }
            let c = compare_ignoring_case(self.host.as_deref(), other.host.as_deref());
            if c != 0 {
                return c;
            }
            return self.port.wrapping_sub(other.port);
        }
        uri_compare(self.authority.as_deref(), other.authority.as_deref())
    }
}

/// A URI syntax failure: the `URISyntaxException` reason, the
/// UTF-16 index (when the JDK reports one), and the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UriError {
    /// `URISyntaxException`: reason + optional index + input.
    Syntax {
        /// The JDK reason (`Illegal character in path`, ...).
        reason: String,
        /// The UTF-16 index, when reported.
        index: Option<usize>,
        /// The offending input.
        input: String,
    },
}

impl UriError {
    fn syntax(reason: &str, index: Option<usize>, input: &str) -> Self {
        Self::Syntax {
            reason: reason.to_owned(),
            index,
            input: input.to_owned(),
        }
    }

    fn expecting(what: &str, index: usize, input: &str) -> Self {
        Self::syntax(&format!("Expected {what}"), Some(index), input)
    }
}

impl std::fmt::Display for UriError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::Syntax {
            reason,
            index,
            input,
        } = self;
        match index {
            Some(i) => write!(f, "{reason} at index {i}: {input}"),
            None => write!(f, "{reason}: {input}"),
        }
    }
}

impl std::error::Error for UriError {}

// -- RFC 2396 character masks (the JDK's longs, composed like the
// originals; bit `i` set iff the ASCII char `i` is permitted).
// The zero bit (`M_ESCAPED`) marks components that allow `%`
// escapes and visible non-ASCII (handled in `scan_escape`).

const L_DIGIT: u64 = 0x3FF000000000000;
const H_DIGIT: u64 = 0;
const H_UPALPHA: u64 = 0x7FFFFFE;
const H_LOWALPHA: u64 = 0x7FFFFFE00000000;
const L_ALPHA: u64 = 0;
const H_ALPHA: u64 = H_LOWALPHA | H_UPALPHA;
const L_ALPHANUM: u64 = L_DIGIT | L_ALPHA;
const H_ALPHANUM: u64 = H_DIGIT | H_ALPHA;
const L_HEX: u64 = L_DIGIT;
const H_HEX: u64 = 0x7E0000007E;
const L_MARK: u64 = 0x678200000000;
const H_MARK: u64 = 0x4000000080000000;
const L_UNRESERVED: u64 = L_ALPHANUM | L_MARK;
const H_UNRESERVED: u64 = H_ALPHANUM | H_MARK;
const L_RESERVED: u64 = 0xAC00985000000000;
const H_RESERVED: u64 = 0x28000001;
const L_ESCAPED: u64 = 1;
const H_ESCAPED: u64 = 0;
const L_URIC: u64 = L_RESERVED | L_UNRESERVED | L_ESCAPED;
const H_URIC: u64 = H_RESERVED | H_UNRESERVED | H_ESCAPED;
const L_PCHAR: u64 = L_UNRESERVED | L_ESCAPED | 0x2400185000000000;
const H_PCHAR: u64 = H_UNRESERVED | H_ESCAPED | 0x1;
const L_PATH: u64 = L_PCHAR | 0x800800000000000;
const H_PATH: u64 = H_PCHAR;
const L_DASH: u64 = 0x200000000000;
const H_DASH: u64 = 0x0;
const L_DOT: u64 = 0x400000000000;
const H_DOT: u64 = 0x0;
const L_USERINFO: u64 = L_UNRESERVED | L_ESCAPED | 0x2C00185000000000;
const H_USERINFO: u64 = H_UNRESERVED | H_ESCAPED;
const L_REG_NAME: u64 = L_UNRESERVED | L_ESCAPED | 0x2C00185000000000;
const H_REG_NAME: u64 = H_UNRESERVED | H_ESCAPED | 0x1;
const L_SERVER: u64 = L_USERINFO | L_ALPHANUM | L_DASH | 0x400400000000000;
const H_SERVER: u64 = H_USERINFO | H_ALPHANUM | H_DASH | 0x28000001;
const L_SERVER_PERCENT: u64 = L_SERVER | 0x2000000000;
const H_SERVER_PERCENT: u64 = H_SERVER;
const L_SCHEME: u64 = L_ALPHA | L_DIGIT | 0x680000000000;
const H_SCHEME: u64 = H_ALPHA | H_DIGIT;
const L_SCOPE_ID: u64 = L_ALPHANUM | 0x400000000000;
const H_SCOPE_ID: u64 = H_ALPHANUM | 0x80000000;

/// Whether a UTF-16 unit is permitted by a mask pair (the JDK's
/// `match`: NUL never matches; non-ASCII never matches here —
/// `scan_escape` handles it).
fn mask_match(unit: u16, low: u64, high: u64) -> bool {
    if unit == 0 {
        return false;
    }
    if unit < 64 {
        return (1u64 << unit) & low != 0;
    }
    if unit < 128 {
        return (1u64 << (unit - 64)) & high != 0;
    }
    false
}

/// US-ASCII lowercase (the JDK's `toLower`).
fn ascii_lower(unit: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

/// US-ASCII uppercase (the JDK's `toUpper`).
fn ascii_upper(unit: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&unit) {
        unit - 32
    } else {
        unit
    }
}

/// The JDK parser over UTF-16 units (error indices are UTF-16).
struct UriParser {
    input: String,
    units: Vec<u16>,
    scheme: Option<Arc<str>>,
    authority: Option<Arc<str>>,
    user_info: Option<Arc<str>>,
    host: Option<Arc<str>>,
    port: i32,
    path: Option<Arc<str>>,
    query: Option<Arc<str>>,
    fragment: Option<Arc<str>>,
    ssp: Option<Arc<str>>,
    opaque: bool,
    ipv6_bytes: u32,
}

impl UriParser {
    fn new(s: &str) -> Self {
        Self {
            input: s.to_owned(),
            units: s.encode_utf16().collect(),
            scheme: None,
            authority: None,
            user_info: None,
            host: None,
            port: -1,
            path: None,
            query: None,
            fragment: None,
            ssp: None,
            opaque: false,
            ipv6_bytes: 0,
        }
    }

    /// Slice UTF-16 units back to a string (the units came from
    /// valid `str`, so every slice is a valid slice).
    fn slice(&self, from: usize, to: usize) -> Arc<str> {
        let start = char_boundary_before(&self.input, from);
        let end = char_boundary_before(&self.input, to);
        Arc::from(&self.input[start..end])
    }

    /// Whether `unit` sits at `pos` (the JDK's `at`).
    fn at(&self, pos: usize, end: usize, unit: u16) -> bool {
        pos < end && self.units[pos] == unit
    }

    /// Scan to the first `stop` unit, failing on `err` units with
    /// -1 (the JDK's `scan(start, end, err, stop)`).
    fn scan_stop(&self, start: usize, end: usize, err: &[u16], stop: &[u16]) -> i64 {
        let mut p = start;
        while p < end {
            let c = self.units[p];
            if err.contains(&c) {
                return -1;
            }
            if stop.contains(&c) {
                break;
            }
            p += 1;
        }
        p as i64
    }

    /// Scan to the first `stop` unit (the JDK's `scan(start, end,
    /// stop)`).
    fn scan_to(&self, start: usize, end: usize, stop: &[u16]) -> usize {
        let mut p = start;
        while p < end {
            if stop.contains(&self.units[p]) {
                break;
            }
            p += 1;
        }
        p
    }

    /// Scan one escape or visible non-ASCII unit (the JDK's
    /// `scanEscape`): a valid `%HH` triple advances 3 (else the
    /// malformed-pair throw); a unit above 128 that is neither a
    /// space char nor a control advances 1; anything else stalls.
    fn scan_escape(&self, start: usize, end: usize) -> Result<usize, UriError> {
        let c = self.units[start];
        if c == u16::from(b'%') {
            if start + 3 <= end
                && mask_match(self.units[start + 1], L_HEX, H_HEX)
                && mask_match(self.units[start + 2], L_HEX, H_HEX)
            {
                return Ok(start + 3);
            }
            return Err(UriError::syntax(
                "Malformed escape pair",
                Some(start),
                &self.input,
            ));
        }
        if c > 128 && !is_space_unit(c) && !is_control_unit(c) {
            return Ok(start + 1);
        }
        Ok(start)
    }

    /// Scan mask-permitted units (the JDK's mask `scan`).
    fn scan_mask(&self, start: usize, end: usize, low: u64, high: u64) -> Result<usize, UriError> {
        let mut p = start;
        while p < end {
            let c = self.units[p];
            if mask_match(c, low, high) {
                p += 1;
                continue;
            }
            if low & L_ESCAPED != 0 {
                let q = self.scan_escape(p, end)?;
                if q > p {
                    p = q;
                    continue;
                }
            }
            break;
        }
        Ok(p)
    }

    /// Check a unit range against masks (the JDK's `checkChars`).
    fn check_chars(
        &self,
        start: usize,
        end: usize,
        low: u64,
        high: u64,
        what: &str,
    ) -> Result<(), UriError> {
        let p = self.scan_mask(start, end, low, high)?;
        if p < end {
            return Err(UriError::syntax(
                &format!("Illegal character in {what}"),
                Some(p),
                &self.input,
            ));
        }
        Ok(())
    }

    /// Check one unit against masks (the JDK's `checkChar`).
    fn check_char(&self, pos: usize, low: u64, high: u64, what: &str) -> Result<(), UriError> {
        self.check_chars(pos, pos + 1, low, high, what)
    }

    /// `[<scheme>:]<ssp>[#<fragment>]` (the JDK's `parse`).
    fn parse(mut self) -> Result<Uri, UriError> {
        let n = self.units.len();
        let p = self.scan_stop(0, n, &units_of("/?#"), &units_of(":"));
        if p >= 0 && self.at(p as usize, n, u16::from(b':')) {
            self.parse_scheme(p as usize)?;
        } else {
            let p = self.parse_hierarchical(0, n)?;
            self.finish(p, n)?;
        }
        Ok(self.into_uri())
    }

    /// The scheme arm of `parse` (split for the budget).
    fn parse_scheme(&mut self, p: usize) -> Result<(), UriError> {
        let n = self.units.len();
        if p == 0 {
            return Err(UriError::expecting("scheme name", 0, &self.input));
        }
        self.check_char(0, L_ALPHA, H_ALPHA, "scheme name")?;
        self.check_chars(1, p, L_SCHEME, H_SCHEME, "scheme name")?;
        self.scheme = Some(self.slice(0, p));
        let mut p = p + 1;
        if self.at(p, n, u16::from(b'/')) {
            p = self.parse_hierarchical(p, n)?;
        } else {
            let q = self.scan_to(p, n, &units_of("#"));
            if q <= p {
                return Err(UriError::expecting("scheme-specific part", p, &self.input));
            }
            self.check_chars(p, q, L_URIC, H_URIC, "opaque part")?;
            self.ssp = Some(self.slice(p, q));
            self.opaque = true;
            p = q;
        }
        self.finish(p, n)
    }

    /// The trailing `#fragment` (split for the budget).
    fn finish(&mut self, p: usize, n: usize) -> Result<(), UriError> {
        if self.at(p, n, u16::from(b'#')) {
            self.check_chars(p + 1, n, L_URIC, H_URIC, "fragment")?;
            self.fragment = Some(self.slice(p + 1, n));
        }
        Ok(())
    }

    /// `[//authority]<path>[?<query>]` (the JDK's
    /// `parseHierarchical`, deviations included).
    fn parse_hierarchical(&mut self, start: usize, n: usize) -> Result<usize, UriError> {
        let mut p = start;
        if self.at(p, n, u16::from(b'/')) && self.at(p + 1, n, u16::from(b'/')) {
            p = self.parse_authority_open(p, n)?;
        }
        let q = self.scan_to(p, n, &units_of("?#"));
        self.check_chars(p, q, L_PATH, H_PATH, "path")?;
        self.path = Some(self.slice(p, q));
        p = q;
        if self.at(p, n, u16::from(b'?')) {
            p += 1;
            let q = self.scan_to(p, n, &units_of("#"));
            self.check_chars(p, q, L_URIC, H_URIC, "query")?;
            self.query = Some(self.slice(p, q));
            p = q;
        }
        Ok(p)
    }

    /// The `//authority` opener (split for the budget): a
    /// non-empty authority parses, an empty one rides only ahead
    /// of more input, and `//` at end-of-input fails.
    fn parse_authority_open(&mut self, p: usize, n: usize) -> Result<usize, UriError> {
        let p = p + 2;
        let q = self.scan_to(p, n, &units_of("/?#"));
        if q > p {
            self.parse_authority(p, q)?;
            Ok(q)
        } else if q < n {
            Ok(p)
        } else {
            Err(UriError::expecting("authority", p, &self.input))
        }
    }

    /// Build the [`Uri`] value (split for the budget).
    fn into_uri(self) -> Uri {
        Uri {
            text: Arc::from(self.input.as_str()),
            scheme: self.scheme,
            authority: self.authority,
            user_info: self.user_info,
            host: self.host,
            port: self.port,
            path: self.path,
            query: self.query,
            fragment: self.fragment,
            opaque: self.opaque,
            ssp: self.ssp,
            id: URI_ID.fetch_add(1, AtomicOrdering::Relaxed),
        }
    }

    /// `authority = server | reg_name` (the JDK's `parseAuthority`):
    /// server chars attempt the server parse (syntax failures fall
    /// back to registry; an escaping `NumberFormatException` keeps
    /// flying — the JDK catches `URISyntaxException` only);
    /// anything else falls straight to registry (the `REG_NAME`
    /// masks sit inside the `SERVER` masks, so a `reg && !server`
    /// fast path would be dead — the fallback installs it).
    fn parse_authority(&mut self, start: usize, end: usize) -> Result<(), UriError> {
        let server_chars = self.authority_server_chars(start, end)?;
        let qreg = self.scan_mask(start, end, L_REG_NAME, H_REG_NAME)?;
        let reg_chars = qreg == end;
        let mut saved: Option<UriError> = None;
        if server_chars {
            match self.parse_server(start, end) {
                Ok(q) => {
                    debug_assert_eq!(q, end, "parse_server throws rather than short-returning");
                    self.authority = Some(self.slice(start, end));
                    return Ok(());
                }
                Err(e) => {
                    self.user_info = None;
                    self.host = None;
                    self.port = -1;
                    saved = Some(e);
                }
            }
        }
        self.authority_fallback(start, end, qreg, reg_chars, saved)
    }

    /// Whether the authority scans as server chars (split for the
    /// budget): `%` rides only beside a bracketed IPv6 literal.
    fn authority_server_chars(&self, start: usize, end: usize) -> Result<bool, UriError> {
        let bracketed = self.scan_to(start, end, &units_of("]")) > start;
        let (low, high) = if bracketed {
            (L_SERVER_PERCENT, H_SERVER_PERCENT)
        } else {
            (L_SERVER, H_SERVER)
        };
        Ok(self.scan_mask(start, end, low, high)? == end)
    }

    /// The registry fallback of `parse_authority` (split for the
    /// budget): registry chars install, else the saved server
    /// error throws, else the authority opener fails at the
    /// registry scan's stall.
    fn authority_fallback(
        &mut self,
        start: usize,
        end: usize,
        qreg: usize,
        reg_chars: bool,
        saved: Option<UriError>,
    ) -> Result<(), UriError> {
        if reg_chars {
            self.authority = Some(self.slice(start, end));
            return Ok(());
        }
        if let Some(e) = saved {
            return Err(e);
        }
        Err(UriError::syntax(
            "Illegal character in authority",
            Some(qreg),
            &self.input,
        ))
    }

    /// `[<userinfo>@]<host>[:<port>]` (the JDK's `parseServer`).
    fn parse_server(&mut self, start: usize, end: usize) -> Result<usize, UriError> {
        let mut p = start;
        let q = self.scan_stop(p, end, &units_of("/?#"), &units_of("@"));
        if q >= p as i64 && self.at(q as usize, end, u16::from(b'@')) {
            let q = q as usize;
            self.check_chars(p, q, L_USERINFO, H_USERINFO, "user info")?;
            self.user_info = Some(self.slice(p, q));
            p = q + 1;
        }
        p = self.parse_server_host(p, end)?;
        p = self.parse_server_port(p, end)?;
        if p < end {
            return Err(UriError::expecting("port number", p, &self.input));
        }
        Ok(p)
    }

    /// The host leg of `parse_server` (split for the budget):
    /// bracketed IPv6 (strict), else IPv4, else hostname.
    fn parse_server_host(&mut self, p: usize, end: usize) -> Result<usize, UriError> {
        if self.at(p, end, u16::from(b'[')) {
            return self.parse_bracketed(p, end);
        }
        let q = self.parse_ipv4(p, end);
        if q > p as i64 {
            return Ok(q as usize);
        }
        self.parse_hostname(p, end)
    }

    /// A bracketed IPv6 literal with optional `%scope` (split for
    /// the budget; failures throw — no registry fallback past the
    /// bracket).
    fn parse_bracketed(&mut self, p: usize, end: usize) -> Result<usize, UriError> {
        let p = p + 1;
        let q = self.scan_stop(p, end, &units_of("/?#"), &units_of("]"));
        if q > p as i64 && self.at(q as usize, end, u16::from(b']')) {
            let q = q as usize;
            let r = self.scan_to(p, q, &units_of("%"));
            if r > p {
                self.parse_ipv6(p, r)?;
                if r + 1 == q {
                    return Err(UriError::syntax("scope id expected", None, &self.input));
                }
                self.check_chars(r + 1, q, L_SCOPE_ID, H_SCOPE_ID, "scope id")?;
            } else {
                self.parse_ipv6(p, q)?;
            }
            self.host = Some(self.slice(p - 1, q + 1));
            return Ok(q + 1);
        }
        Err(UriError::expecting(
            "closing bracket for IPv6 address",
            q.max(p as i64) as usize,
            &self.input,
        ))
    }

    /// The port leg of `parse_server` (split for the budget): ASCII
    /// digits (an empty port leaves -1); overflow fails the server
    /// parse toward registry.
    fn parse_server_port(&mut self, p: usize, end: usize) -> Result<usize, UriError> {
        let mut p = p;
        if self.at(p, end, u16::from(b':')) {
            p += 1;
            let q = self.scan_to(p, end, &units_of("/"));
            if q > p {
                self.check_chars(p, q, L_DIGIT, H_DIGIT, "port number")?;
                let digits: String = self.units[p..q]
                    .iter()
                    .map(|u| char::from_u32(u32::from(*u)).unwrap_or('\u{FFFD}'))
                    .collect();
                match digits.parse::<i32>() {
                    Ok(port) => self.port = port,
                    Err(_) => {
                        return Err(UriError::syntax(
                            "Malformed port number",
                            Some(p),
                            &self.input,
                        ));
                    }
                }
                p = q;
            }
        }
        Ok(p)
    }

    /// Attempt an IPv4 address, else -1 (the JDK's
    /// `parseIPv4Address`): malformed scans and `parseInt` escapes
    /// both miss toward hostname; a trailing non-`:` misses too.
    /// A hit installs the host.
    fn parse_ipv4(&mut self, start: usize, end: usize) -> i64 {
        // Lenient scans miss (never throw), so the fallback is dead
        // by construction — but total either way.
        let p = self.scan_ipv4(start, end, false).unwrap_or(-1);
        if p > start as i64 && p < end as i64 && !self.at(p as usize, end, u16::from(b':')) {
            return -1;
        }
        if p > start as i64 {
            self.host = Some(self.slice(start, p as usize));
        }
        p
    }

    /// Scan an IPv4 address (the JDK's `scanIPv4Address`): -1 when
    /// the interval holds no digits-and-dots (or, strict, anything
    /// else); otherwise three byte/dot rounds plus the last byte —
    /// a stall fails malformed under strict, misses lenient.
    fn scan_ipv4(&self, start: usize, end: usize, strict: bool) -> Result<i64, UriError> {
        let m = self.scan_mask_plain(start, end, L_DIGIT | L_DOT, H_DIGIT | H_DOT);
        if m <= start || (strict && m != end) {
            return Ok(-1);
        }
        let mut p = start;
        for _ in 0..3 {
            let q = self.scan_byte(p, m);
            if q <= p {
                return self.ipv4_stall(q, strict);
            }
            if !self.at(q, m, u16::from(b'.')) {
                return self.ipv4_stall(q, strict);
            }
            p = q + 1;
        }
        let q = self.scan_byte(p, m);
        if q <= p || q < m {
            return self.ipv4_stall(q.max(p), strict);
        }
        Ok(q as i64)
    }

    /// An IPv4 stall (split for the budget): the malformed throw
    /// under strict, a miss otherwise.
    fn ipv4_stall(&self, q: usize, strict: bool) -> Result<i64, UriError> {
        if strict {
            return Err(UriError::syntax(
                "Malformed IPv4 address",
                Some(q),
                &self.input,
            ));
        }
        Ok(-1)
    }

    /// Scan mask-permitted units without escape handling (for
    /// masks lacking the escaped bit — infallible by
    /// construction).
    fn scan_mask_plain(&self, start: usize, end: usize, low: u64, high: u64) -> usize {
        debug_assert_eq!(low & L_ESCAPED, 0, "plain scans take escape-less masks");
        let mut p = start;
        while p < end && mask_match(self.units[p], low, high) {
            p += 1;
        }
        p
    }

    /// Scan a decimal byte (the JDK's `scanByte`): no digits
    /// stalls; fewer than three significant digits (past leading
    /// zeros) always fit; more than three never do; exactly three
    /// parse (infallibly — three digits cannot overflow).
    fn scan_byte(&self, start: usize, end: usize) -> usize {
        let q = self.scan_mask_plain(start, end, L_DIGIT, H_DIGIT);
        if q <= start {
            return q;
        }
        let mut i = start;
        while i < q && self.units[i] == u16::from(b'0') {
            i += 1;
        }
        let significant = q - i;
        if significant < 3 {
            return q;
        }
        if significant > 3 {
            return start;
        }
        let digits: String = self.units[start..q]
            .iter()
            .map(|u| char::from_u32(u32::from(*u)).unwrap_or('\u{FFFD}'))
            .collect();
        match digits.parse::<u32>() {
            Ok(v) if v <= 255 => q,
            _ => start,
        }
    }

    /// Take an IPv4 address or fail (the JDK's `takeIPv4Address`).
    fn take_ipv4(&self, start: usize, end: usize, expected: &str) -> Result<usize, UriError> {
        match self.scan_ipv4(start, end, true) {
            Ok(p) if p > start as i64 => Ok(p as usize),
            Ok(_) => Err(UriError::expecting(expected, start, &self.input)),
            Err(e) => Err(e),
        }
    }

    /// `hostname = domainlabel["."] | 1*(domainlabel".")toplabel["."]`
    /// (the JDK's `parseHostname`): labels of alphanum/dash runs,
    /// the rightmost multi-label start alpha-checked.
    fn parse_hostname(&mut self, start: usize, end: usize) -> Result<usize, UriError> {
        let mut p = start;
        let mut last = -1i64;
        loop {
            let q = self.scan_mask_plain(p, end, L_ALPHANUM, H_ALPHANUM);
            if q <= p {
                break;
            }
            last = p as i64;
            p = q;
            let q = self.scan_mask_plain(p, end, L_ALPHANUM | L_DASH, H_ALPHANUM | H_DASH);
            if q > p {
                if self.units[q - 1] == u16::from(b'-') {
                    return Err(UriError::syntax(
                        "Illegal character in hostname",
                        Some(q - 1),
                        &self.input,
                    ));
                }
                p = q;
            }
            if self.at(p, end, u16::from(b'.')) {
                p += 1;
            } else {
                break;
            }
            if p >= end {
                break;
            }
        }
        if p < end && !self.at(p, end, u16::from(b':')) {
            return Err(UriError::syntax(
                "Illegal character in hostname",
                Some(p),
                &self.input,
            ));
        }
        if last < 0 {
            return Err(UriError::expecting("hostname", start, &self.input));
        }
        if last > start as i64 && !mask_match(self.units[last as usize], L_ALPHA, H_ALPHA) {
            return Err(UriError::syntax(
                "Illegal character in hostname",
                Some(last as usize),
                &self.input,
            ));
        }
        self.host = Some(self.slice(start, p));
        Ok(p)
    }

    /// An IPv6 reference (the JDK's `parseIPv6Reference`): hex
    /// runs with at most one `::`, an optional IPv4 tail, and the
    /// 16-byte budget.
    fn parse_ipv6(&mut self, start: usize, end: usize) -> Result<usize, UriError> {
        let mut p = start;
        let mut compressed = false;
        let q = self.scan_hex_seq(p, end)?;
        if q > p as i64 {
            p = q as usize;
            if self.at2(p, end, "::") {
                compressed = true;
                p = self.scan_hex_post(p + 2, end)?;
            } else if self.at(p, end, u16::from(b':')) {
                let tail = self.take_ipv4(p + 1, end, "IPv4 address")?;
                self.ipv6_bytes += 4;
                p = tail;
            }
        } else if self.at2(p, end, "::") {
            compressed = true;
            p = self.scan_hex_post(p + 2, end)?;
        }
        if p < end {
            return Err(UriError::syntax(
                "Malformed IPv6 address",
                Some(start),
                &self.input,
            ));
        }
        if self.ipv6_bytes > 16 {
            return Err(UriError::syntax(
                "IPv6 address too long",
                Some(start),
                &self.input,
            ));
        }
        if !compressed && self.ipv6_bytes < 16 {
            return Err(UriError::syntax(
                "IPv6 address too short",
                Some(start),
                &self.input,
            ));
        }
        if compressed && self.ipv6_bytes == 16 {
            return Err(UriError::syntax(
                "Malformed IPv6 address",
                Some(start),
                &self.input,
            ));
        }
        Ok(p)
    }

    /// Whether the two units at `pos` spell `s` (the JDK's string
    /// `at` for the `::` probes).
    fn at2(&self, pos: usize, end: usize, s: &str) -> bool {
        let want: Vec<u16> = units_of(s);
        if want.len() > end - pos.min(end) {
            return false;
        }
        self.units[pos..pos + want.len()] == want[..]
    }

    /// The post-`::` run (the JDK's `scanHexPost`).
    fn scan_hex_post(&mut self, start: usize, end: usize) -> Result<usize, UriError> {
        if start == end {
            return Ok(start);
        }
        let p = start;
        let q = self.scan_hex_seq(p, end)?;
        if q > p as i64 {
            let mut p = q as usize;
            if self.at(p, end, u16::from(b':')) {
                p += 1;
                p = self.take_ipv4(p, end, "hex digits or IPv4 address")?;
                self.ipv6_bytes += 4;
            }
            Ok(p)
        } else {
            let p = self.take_ipv4(p, end, "hex digits or IPv4 address")?;
            self.ipv6_bytes += 4;
            Ok(p)
        }
    }

    /// One hex run (the JDK's `scanHexSeq`): 1–4 hex digits per
    /// group, single-`:` separated, stopping ahead of `::` or an
    /// IPv4 tail. Returns -1 when no run opens.
    fn scan_hex_seq(&mut self, start: usize, end: usize) -> Result<i64, UriError> {
        let mut p = start;
        let mut q = self.scan_mask_plain(p, end, L_HEX, H_HEX);
        if q <= p {
            return Ok(-1);
        }
        if self.at(q, end, u16::from(b'.')) {
            return Ok(-1);
        }
        if q > p + 4 {
            return Err(UriError::syntax(
                "IPv6 hexadecimal digit sequence too long",
                Some(p),
                &self.input,
            ));
        }
        self.ipv6_bytes += 2;
        p = q;
        while p < end {
            if !self.at(p, end, u16::from(b':')) {
                break;
            }
            if self.at(p + 1, end, u16::from(b':')) {
                break;
            }
            p += 1;
            q = self.scan_mask_plain(p, end, L_HEX, H_HEX);
            if q <= p {
                return Err(UriError::expecting(
                    "digits for an IPv6 address",
                    p,
                    &self.input,
                ));
            }
            if self.at(q, end, u16::from(b'.')) {
                p -= 1;
                break;
            }
            if q > p + 4 {
                return Err(UriError::syntax(
                    "IPv6 hexadecimal digit sequence too long",
                    Some(p),
                    &self.input,
                ));
            }
            self.ipv6_bytes += 2;
            p = q;
        }
        Ok(p as i64)
    }
}

/// Encode a delimiter string as UTF-16 units (split for the
/// budget).
fn units_of(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// The byte index of a UTF-16 offset, floored to a char boundary
/// (split for the budget; the parser only slices at ASCII
/// delimiters, so the floor never bites).
fn char_boundary_before(input: &str, utf16pos: usize) -> usize {
    let mut utf16 = 0usize;
    let mut floor = 0usize;
    for (byte, c) in input.char_indices() {
        if utf16 == utf16pos {
            return byte;
        }
        if utf16 > utf16pos {
            return floor;
        }
        floor = byte;
        utf16 += c.len_utf16();
    }
    input.len()
}

/// Whether a UTF-16 unit is a Java space char (split for the
/// budget; only called above 128, where `is_whitespace` agrees
/// with `isSpaceChar` — U+0085 reads whitespace on both sides of
/// the verdict through `is_control_unit`).
fn is_space_unit(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(|c| c.is_whitespace())
}

/// Whether a UTF-16 unit is a Java ISO control (split for the
/// budget; surrogates read false, like the JDK's per-unit scan).
fn is_control_unit(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(|c| c.is_control())
}

/// UTF-16 units of a part (split for the budget).
fn part_units(part: Option<&str>) -> Option<Vec<u16>> {
    part.map(|s| s.encode_utf16().collect())
}

/// `compareTo` over UTF-16 units (the JDK's `String.compareTo`):
/// first differing units as an `i32` diff, else the length diff.
fn units_compare(s: &[u16], t: &[u16]) -> i32 {
    let n = s.len().min(t.len());
    for i in 0..n {
        let diff = i32::from(s[i]) - i32::from(t[i]);
        if diff != 0 {
            return diff;
        }
    }
    s.len() as i32 - t.len() as i32
}

/// Component equality (the JDK's `equal`): the percent-normalized
/// comparison reads zero.
fn uri_equal(s: Option<&str>, t: Option<&str>) -> bool {
    percent_normalized(&part_units(s), &part_units(t), true) == 0
}

/// Component ordering (the JDK's `compare`).
fn uri_compare(s: Option<&str>, t: Option<&str>) -> i32 {
    percent_normalized(&part_units(s), &part_units(t), false)
}

/// The JDK's `percentNormalizedComparison`: `%`-escapes compare by
/// lowercase hex; anything else compares raw. NOTE the asymmetry:
/// a `%`-free `s` takes the plain `compareTo` even when `t` holds
/// escapes.
fn percent_normalized(s: &Option<Vec<u16>>, t: &Option<Vec<u16>>, equality: bool) -> i32 {
    let (Some(s), Some(t)) = (s, t) else {
        // `(Some, Some)` is excluded above, so matching on `s`
        // alone is exhaustive with no dead arm.
        return match s {
            None if t.is_none() => 0,
            None => -1,
            Some(_) => 1,
        };
    };
    if !s.contains(&u16::from(b'%')) {
        return units_compare(s, t);
    }
    if s.len() != t.len() && equality {
        return s.len() as i32 - t.len() as i32;
    }
    percent_loop(s, t)
}

/// The normalized comparison loop (split for the budget). Units
/// past a `%` index directly: both sides are parsed, so every `%`
/// pairs (the same invariant the JDK relies on).
fn percent_loop(s: &[u16], t: &[u16]) -> i32 {
    let mut i = 0usize;
    let n = s.len().min(t.len());
    while i < n {
        let (c, d) = (s[i], t[i]);
        let val = i32::from(c) - i32::from(d);
        if c != u16::from(b'%') {
            if val != 0 {
                return val;
            }
            i += 1;
            continue;
        }
        if d != u16::from(b'%') {
            return val;
        }
        i += 1;
        let hex1 = i32::from(ascii_lower(s[i])) - i32::from(ascii_lower(t[i]));
        if hex1 != 0 {
            return hex1;
        }
        i += 1;
        let hex2 = i32::from(ascii_lower(s[i])) - i32::from(ascii_lower(t[i]));
        if hex2 != 0 {
            return hex2;
        }
        i += 1;
    }
    s.len() as i32 - t.len() as i32
}

/// Case-insensitive component equality (the JDK's
/// `equalIgnoringCase`; two undefined components read equal).
fn equal_ignoring_case(s: Option<&str>, t: Option<&str>) -> bool {
    match (part_units(s), part_units(t)) {
        (None, None) => true,
        (Some(s), Some(t)) => {
            s.len() == t.len()
                && s.iter()
                    .zip(t.iter())
                    .all(|(c, d)| ascii_lower(*c) == ascii_lower(*d))
        }
        _ => false,
    }
}

/// Case-insensitive component ordering (the JDK's
/// `compareIgnoringCase`).
fn compare_ignoring_case(s: Option<&str>, t: Option<&str>) -> i32 {
    match (part_units(s), part_units(t)) {
        (None, None) => 0,
        (None, Some(_)) => -1,
        (Some(_), None) => 1,
        (Some(s), Some(t)) => {
            let n = s.len().min(t.len());
            for i in 0..n {
                let diff = i32::from(ascii_lower(s[i])) - i32::from(ascii_lower(t[i]));
                if diff != 0 {
                    return diff;
                }
            }
            s.len() as i32 - t.len() as i32
        }
    }
}

/// `String.hashCode` over UTF-16 units (split for the budget).
fn java_str_hash(units: &[u16]) -> i32 {
    let mut h = 0i32;
    for u in units {
        h = h.wrapping_mul(31).wrapping_add(i32::from(*u));
    }
    h
}

/// Component hashing (the JDK's `hash`): undefined passes the
/// accumulator through, `%`-free parts mix `String.hashCode`
/// under 127, escaped parts normalize first.
fn uri_hash(hash: i32, part: Option<&str>) -> i32 {
    let Some(units) = part_units(part) else {
        return hash;
    };
    if !units.contains(&u16::from(b'%')) {
        return hash.wrapping_mul(127).wrapping_add(java_str_hash(&units));
    }
    normalized_hash(hash, &units)
}

/// Escape-normalizing hash (the JDK's `normalizedHash`): hex
/// digits past `%` hash uppercased. Units past `%` index directly
/// (parsed escapes always pair — see [`percent_loop`]).
fn normalized_hash(hash: i32, units: &[u16]) -> i32 {
    let mut h = 0i32;
    let mut i = 0usize;
    while i < units.len() {
        let c = units[i];
        h = h.wrapping_mul(31).wrapping_add(i32::from(c));
        if c == u16::from(b'%') {
            h = h
                .wrapping_mul(31)
                .wrapping_add(i32::from(ascii_upper(units[i + 1])));
            h = h
                .wrapping_mul(31)
                .wrapping_add(i32::from(ascii_upper(units[i + 2])));
            i += 2;
        }
        i += 1;
    }
    hash.wrapping_mul(127).wrapping_add(h)
}

/// One normalized part for [`Uri::hash_content`] (split for the
/// budget): `None`/empty discriminate, units length-prefix, hex
/// past `%` lowers (parsed escapes always pair — see
/// [`percent_loop`]).
fn hash_part<H: std::hash::Hasher>(state: &mut H, part: Option<&str>, full_lower: bool) {
    let Some(s) = part else {
        state.write_u8(0);
        return;
    };
    state.write_u8(1);
    let units: Vec<u16> = s.encode_utf16().collect();
    state.write_usize(units.len());
    let mut i = 0usize;
    while i < units.len() {
        let mut u = units[i];
        if full_lower {
            u = ascii_lower(u);
        } else if u == u16::from(b'%') {
            state.write_u16(u);
            state.write_u16(ascii_lower(units[i + 1]));
            state.write_u16(ascii_lower(units[i + 2]));
            i += 3;
            continue;
        }
        state.write_u16(u);
        i += 1;
    }
}

/// Case-insensitive component hashing (the JDK's
/// `hashIgnoringCase`).
fn hash_ignoring_case(hash: i32, part: Option<&str>) -> i32 {
    let Some(units) = part_units(part) else {
        return hash;
    };
    let mut h = hash;
    for u in &units {
        h = h.wrapping_mul(31).wrapping_add(i32::from(ascii_lower(*u)));
    }
    h
}

/// Millis → rendered fields (cutover-aware civil date, Sunday-0
/// weekday straight from the day count).
fn fields_from_millis(millis: i64) -> DateFields {
    let days = millis.div_euclid(86_400_000);
    let day_millis = millis.rem_euclid(86_400_000);
    let (year, month, day) = if millis < CUTOVER_MILLIS {
        civil_julian(days)
    } else {
        civil_proleptic(days)
    };
    DateFields {
        year,
        month,
        day,
        hour: (day_millis / 3_600_000) as u32,
        minute: ((day_millis / 60_000) % 60) as u32,
        second: ((day_millis / 1_000) % 60) as u32,
        milli: (day_millis % 1_000) as u32,
        weekday: (days + 4).rem_euclid(7) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::{Instant, Uri, Uuid, char_boundary_before};
    use std::cmp::Ordering;
    use std::str::FromStr;

    fn uuid(s: &str) -> Uuid {
        Uuid::from_str(s).unwrap_or_else(|e| panic!("{s}: {e:?}"))
    }

    fn uuid_err(s: &str) -> (String, String) {
        let err = Uuid::from_str(s).expect_err("expected a UUID error");
        (err.class.to_owned(), err.message)
    }

    /// Canonicalization + lenient masking (probes `u-parse-*`).
    #[test]
    fn uuid_parses_shapes() {
        for (input, want) in [
            (
                "550e8400-e29b-41d4-a716-446655440000",
                "550e8400-e29b-41d4-a716-446655440000",
            ),
            (
                "550E8400-E29B-41D4-A716-446655440000",
                "550e8400-e29b-41d4-a716-446655440000",
            ),
            (
                "550e840-e29b-41d4-a716-446655440000",
                "0550e840-e29b-41d4-a716-446655440000",
            ),
            (
                "550e8400-e29b-41d-a716-446655440000",
                "550e8400-e29b-041d-a716-446655440000",
            ),
            (
                "123456789-123-1234-1234-123456789012",
                "23456789-0123-1234-1234-123456789012",
            ),
            ("0-0-0-0-0", "00000000-0000-0000-0000-000000000000"),
            (
                "+550e840-e29b-41d4-a716-44665544000",
                "0550e840-e29b-41d4-a716-044665544000",
            ),
        ] {
            assert_eq!(uuid(input).to_canonical(), want, "{input}");
        }
    }

    /// The slow-path failures (probes `u-parse-*`, `e-uuid-*`).
    #[test]
    fn uuid_rejects_shapes() {
        for s in [
            "{550e8400-e29b-41d4-a716-446655440000}",
            "urn:uuid:550e8400-e29b-41d4-a716-446655440000",
            "550e8400-e29b-41d4-a716-446655440000-",
            "123456789abcdef0-e29b-41d4-a716-446655440000",
        ] {
            assert_eq!(
                uuid_err(s),
                (
                    "IllegalArgumentException".to_owned(),
                    "UUID string too large".to_owned()
                ),
                "{s}"
            );
        }
        for s in [
            "bogus",
            "",
            "550e8400-e29b-41d4-a716",
            "550e8400e29b41d4a716446655440000",
            "1-2-3-4-5-6",
        ] {
            assert_eq!(
                uuid_err(s),
                (
                    "IllegalArgumentException".to_owned(),
                    format!("Invalid UUID string: {s}")
                ),
                "{s}"
            );
        }
        for s in ["550e8400--41d4-a716-446655440000", "----"] {
            assert_eq!(
                uuid_err(s),
                (
                    "NumberFormatException".to_owned(),
                    "For input string: \"\" under radix 16".to_owned()
                ),
                "{s}"
            );
        }
        assert_eq!(
            uuid_err("g50e8400-e29b-41d4-a716-446655440000"),
            (
                "NumberFormatException".to_owned(),
                "Error at index 0 in: \"g50e8400\"".to_owned()
            )
        );
        assert_eq!(
            uuid_err("ffffffffffffffffff-1-1-1-1"),
            (
                "NumberFormatException".to_owned(),
                "Error at index 15 in: \"ffffffffffffffffff\"".to_owned()
            )
        );
        assert_eq!(
            uuid_err("1-1-1-1-fffffffffffffffff"),
            (
                "NumberFormatException".to_owned(),
                "Error at index 15 in: \"fffffffffffffffff\"".to_owned()
            )
        );
        // A non-Latin-1 nibble misses the fast path (36 units, so
        // the fast path runs), then fails the slow path at its
        // group index.
        assert_eq!(
            uuid_err("550e8400-e29b-41d4-a716-44665544Ā000"),
            (
                "NumberFormatException".to_owned(),
                "Error at index 8 in: \"44665544Ā000\"".to_owned()
            )
        );
        // Seventeen digits without an early `i64` trip overflow the
        // `u64` accumulator instead (`2^60 * 16`).
        assert_eq!(
            uuid_err("10000000000000000-1-1-1-1"),
            (
                "NumberFormatException".to_owned(),
                "Error at index 16 in: \"10000000000000000\"".to_owned()
            )
        );
    }

    /// `hashCode` pins (probes `u-hash-*`).
    #[test]
    fn uuid_hashes() {
        assert_eq!(
            uuid("550e8400-e29b-41d4-a716-446655440000").hash_code(),
            1170702770
        );
        assert_eq!(uuid("00000000-0000-0000-0000-000000000000").hash_code(), 0);
        assert_eq!(uuid("ffffffff-ffff-ffff-ffff-ffffffffffff").hash_code(), 0);
        assert_eq!(
            uuid("550E8400-E29B-41D4-A716-446655440000").hash_code(),
            uuid("550e8400-e29b-41d4-a716-446655440000").hash_code()
        );
    }

    /// Signed `(hi, lo)` ordering (probes `u-cmp-*`).
    #[test]
    fn uuid_orders() {
        let ord = |a: &str, b: &str| uuid(a).cmp_uuid(&uuid(b));
        assert_eq!(
            ord(
                "550e8400-e29b-41d4-a716-446655440000",
                "550E8400-E29B-41D4-A716-446655440000"
            ),
            Ordering::Equal
        );
        assert_eq!(
            ord(
                "00000000-0000-0000-0000-000000000001",
                "00000000-0000-0000-0000-000000000002"
            ),
            Ordering::Less
        );
        assert_eq!(
            ord(
                "00000001-0000-0000-0000-000000000000",
                "00000000-ffff-ffff-ffff-ffffffffffff"
            ),
            Ordering::Greater
        );
        assert_eq!(
            ord(
                "ffffffff-ffff-ffff-ffff-ffffffffffff",
                "00000000-0000-0000-0000-000000000000"
            ),
            Ordering::Less
        );
        assert_eq!(
            ord(
                "00000000-0000-0000-ffff-ffffffffffff",
                "00000000-0000-0000-0000-000000000000"
            ),
            Ordering::Less
        );
    }

    /// Version-4 / Leach-Salz stamping (probe `e-uuid-ver4`).
    #[test]
    fn uuid_random_stamps_bits() {
        for (hi, lo) in [
            (0u64, 0u64),
            (u64::MAX, u64::MAX),
            (0x1234_5678_9abc_def0, 0x0fed_cba9_8765_4321),
        ] {
            let u = Uuid::from_random(hi, lo);
            let s = u.to_canonical();
            assert_eq!(s.chars().nth(14), Some('4'), "{s}");
            assert!(
                matches!(s.chars().nth(19), Some('8' | '9' | 'a' | 'b')),
                "{s}"
            );
            assert_eq!(s.len(), 36);
        }
        assert_eq!(
            Uuid::from_random(0, 0).to_canonical(),
            "00000000-0000-4000-8000-000000000000"
        );
    }

    /// `parseLong` edges, covered directly (unreachable through
    /// `from_str`: `-` never survives the dash split).
    #[test]
    fn uuid_group_edges() {
        use super::{GroupError, parse_group, parse_group_chars};
        assert_eq!(parse_group_chars(&[]), Err(GroupError::Empty));
        assert_eq!(parse_group_chars(&['+']), Err(GroupError::Empty));
        assert_eq!(parse_group_chars(&['-', '1']), Ok(-1));
        assert_eq!(
            parse_group_chars(&"8000000000000000".chars().collect::<Vec<_>>()),
            Err(GroupError::BadIndex(15))
        );
        assert_eq!(
            parse_group_chars(&"-8000000000000000".chars().collect::<Vec<_>>()),
            Ok(i64::MIN)
        );
        assert_eq!(
            parse_group_chars(&"-8000000000000001".chars().collect::<Vec<_>>()),
            Err(GroupError::BadIndex(16))
        );
        // Astral input fails at its UTF-16 index (the surrogate
        // halves are non-hex units).
        let astral: Vec<u16> = "😀g".encode_utf16().collect();
        assert_eq!(parse_group(&astral), Err(GroupError::BadIndex(0)));
    }

    /// `char_boundary_before` floors mid-pair offsets (the parser
    /// only slices at ASCII delimiters, so this never bites).
    #[test]
    fn utf16_floor_never_panics() {
        assert_eq!(char_boundary_before("a😀b", 0), 0);
        assert_eq!(char_boundary_before("a😀b", 1), 1);
        assert_eq!(char_boundary_before("a😀b", 2), 1);
        assert_eq!(char_boundary_before("a😀b", 3), 5);
        assert_eq!(char_boundary_before("a😀b", 99), 6);
    }

    fn millis(s: &str) -> i64 {
        Instant::from_str(s)
            .unwrap_or_else(|e| panic!("{s}: {e:?}"))
            .millis
    }

    /// Millis vectors (probes `i-ms-*`).
    #[test]
    fn instant_millis_vectors() {
        for (input, want) in [
            ("2024-01-01", 1704067200000),
            ("2024-01-01T10:00:00.000+02:00", 1704096000000),
            ("2024-01-01T00:00:00", 1704067200000),
            ("2024-01", 1704067200000),
            ("2024", 1704067200000),
            ("2024-01-01T00:00", 1704067200000),
            ("2024-01-01T00:00:00Z", 1704067200000),
            ("2024-01-01T00:00:00.1234567", 1704067200123),
            ("2024-01-01T00:00:00.1", 1704067200100),
            ("2024-01-01T00:00:00.9999", 1704067200999),
            ("2024-01-01T00:59:60", 1704070800000),
            ("2024Z", 1704067200000),
            ("2024-01Z", 1704067200000),
            ("2024-01-01+02:00", 1704060000000),
            ("2024-01-01T00:00:00-05:30", 1704087000000),
            ("2024-02-29", 1709164800000),
            ("2000-02-29", 951782400000),
            ("1960-05-05", -304819200000),
            ("1960-05-05T12:30:15.250-04:00", -304759784750),
            ("0001-01-01", -62135769600000),
            ("1970-01-01T00:00:00.000-00:00", 0),
            ("9999-12-31T23:59:59.999Z", 253402300799999),
        ] {
            assert_eq!(millis(input), want, "{input}");
        }
    }

    /// The cutover region (probes `e-cut-*`).
    #[test]
    fn instant_cutover_vectors() {
        for (input, want) in [
            ("1582-10-04", -12219379200000),
            ("1582-10-15", -12219292800000),
            ("1582-10-10", -12218860800000),
            ("1582-10-05", -12219292800000),
            ("1582-10-14", -12218515200000),
            ("1500-01-01", -14830992000000),
            ("1200-02-29", -24293174400000),
            ("1000-01-01", -30609792000000),
            // Year 0000 reads pure Julian (deliberate: the JVM's
            // year-0 reads are erratic — `f-sw-dec31` lands a day
            // *before* `f-y0-jan` — so the port keeps the
            // documented calendar rules; printing year-0-range
            // millis still matches, `f-y0-date-ctor`).
            ("0000-06-15", -62153049600000),
        ] {
            assert_eq!(millis(input), want, "{input}");
        }
        // The cutover instant itself.
        assert_eq!(super::CUTOVER_MILLIS, -12219292800000);
        assert_eq!(millis("1582-10-15"), super::CUTOVER_MILLIS);
        // Proleptic validation rejects Julian-1500's leap day.
        assert!(Instant::from_str("1500-02-29").is_err());
    }

    /// `Date.hashCode` pins (probes `i-hash-*`).
    #[test]
    fn instant_hashes() {
        assert_eq!(Instant::from_millis(1704067200000).hash_code(), -1034816116);
        assert_eq!(Instant::from_millis(0).hash_code(), 0);
        assert_eq!(Instant::from_millis(-304819200000).hash_code(), -123478087);
        assert_eq!(
            Instant::from_str("2024-01-01T02:00:00+02:00")
                .unwrap()
                .hash_code(),
            Instant::from_str("2024-01-01T00:00:00Z")
                .unwrap()
                .hash_code()
        );
    }

    /// Millis ordering (probes `i-cmp-*`).
    #[test]
    fn instant_orders() {
        let ord = |a: i64, b: i64| Instant::from_millis(a).cmp_instant(&Instant::from_millis(b));
        assert_eq!(ord(1704067200000, 1704067200000), Ordering::Equal);
        assert_eq!(ord(1704067200000, 1704153600000), Ordering::Less);
        assert_eq!(ord(1704153600000, 1704067200000), Ordering::Greater);
        assert_eq!(ord(-304819200000, 1704067200000), Ordering::Less);
    }

    /// `#inst` payloads (probes `i-pr-*`, `e-cut-pr-*`).
    #[test]
    fn instant_prints_rfc3339() {
        for (input, want) in [
            ("2024-01-01", "2024-01-01T00:00:00.000"),
            ("2024-01-01T00:00:00.1234567", "2024-01-01T00:00:00.123"),
            ("1960-05-05", "1960-05-05T00:00:00.000"),
            ("0001-01-01", "0001-01-01T00:00:00.000"),
            ("2024-01-01T10:00:00.000+02:00", "2024-01-01T08:00:00.000"),
            ("1582-10-10", "1582-10-20T00:00:00.000"),
            ("0000-06-15", "0001-06-15T00:00:00.000"),
            ("1500-06-15", "1500-06-15T00:00:00.000"),
        ] {
            assert_eq!(
                Instant::from_str(input).unwrap().to_rfc3339(),
                want,
                "{input}"
            );
        }
        assert_eq!(
            Instant::from_millis(-62135596800000).to_rfc3339(),
            "0001-01-03T00:00:00.000"
        );
        // `yyyy` pads but never truncates (year 10000 reachable via
        // a negative zone on 9999-12-31).
        assert_eq!(
            Instant::from_str("9999-12-31T23:59:00-00:01")
                .unwrap()
                .to_rfc3339(),
            "10000-01-01T00:00:00.000"
        );
    }

    /// `Date.toString` at UTC (probes `i-str-*`, `e-str-*`).
    #[test]
    fn instant_str_is_date_string() {
        for (input, want) in [
            ("2024-01-01", "Mon Jan 01 00:00:00 UTC 2024"),
            (
                "1960-05-05T12:30:15.250-04:00",
                "Thu May 05 16:30:15 UTC 1960",
            ),
            ("0001-01-01", "Sat Jan 01 00:00:00 UTC 1"),
            ("0099-06-15", "Sat Jun 15 00:00:00 UTC 99"),
            // Pure-Julian year-0 millis (the JVM reads "0000-06-15"
            // erratically; its rendering of *these* millis matches
            // — probe `f-y0-str-ctor`).
            ("0000-06-15", "Tue Jun 15 00:00:00 UTC 1"),
            ("1500-06-15", "Mon Jun 15 00:00:00 UTC 1500"),
            ("1582-10-10", "Wed Oct 20 00:00:00 UTC 1582"),
            ("1582-10-04", "Thu Oct 04 00:00:00 UTC 1582"),
            ("2024-01-01T00:00:00.123Z", "Mon Jan 01 00:00:00 UTC 2024"),
        ] {
            assert_eq!(
                Instant::from_str(input).unwrap().to_date_string(),
                want,
                "{input}"
            );
        }
    }

    /// Every S6-valid shape splits (the walk mirrors `parse_instant`;
    /// drift fails here, loudly, before millis go wrong).
    #[test]
    fn validated_shapes_all_split() {
        use super::parse_validated;
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
            "2024-01+02:00",
            "2024-01-01-02:00",
            "9999-12-31T23:59:00-00:01",
        ] {
            assert!(parse_validated(s).is_some(), "{s}");
        }
        // The hour colon is structural (drift trips `from_str`'s
        // expect, never silent millis).
        assert!(parse_validated("2024-01-01T00x00").is_none());
        // A non-digit where digits belong, trailing junk instead of
        // a zone, and a zoneless `+HHMM` (the zone colon) all fail
        // the split.
        assert!(parse_validated("202x").is_none());
        assert!(parse_validated("2024x").is_none());
        assert!(parse_validated("2024+0200").is_none());
    }

    fn uri(s: &str) -> Uri {
        Uri::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn uri_parts(u: &Uri) -> Vec<Option<String>> {
        [
            u.scheme.clone(),
            u.authority.clone(),
            u.user_info.clone(),
            u.host.clone(),
        ]
        .into_iter()
        .map(|p| p.map(|s| s.to_string()))
        .chain([
            Some(u.port.to_string()),
            u.path.clone().map(|s| s.to_string()),
            u.query.clone().map(|s| s.to_string()),
            u.fragment.clone().map(|s| s.to_string()),
            Some(u.opaque.to_string()),
            u.ssp.clone().map(|s| s.to_string()),
        ])
        .collect()
    }

    /// Parsed components (probes `c-*`): raw parts, empty authority
    /// collapsing to null, server/registry splits.
    #[test]
    fn uri_parses_parts() {
        let o = |s: &str| Some(s.to_owned());
        for (input, want) in [
            (
                "http:///x",
                vec![
                    o("http"),
                    None,
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http:/x",
                vec![
                    o("http"),
                    None,
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h:/x",
                vec![
                    o("http"),
                    o("h:"),
                    None,
                    o("h"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h:-1/x",
                vec![
                    o("http"),
                    o("h:-1"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h:8x/x",
                vec![
                    o("http"),
                    o("h:8x"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://[::1]/x",
                vec![
                    o("http"),
                    o("[::1]"),
                    None,
                    o("[::1]"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "HTTP://H/X",
                vec![
                    o("HTTP"),
                    o("H"),
                    None,
                    o("H"),
                    o("-1"),
                    o("/X"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "mailto:a@b.c#f",
                vec![
                    o("mailto"),
                    None,
                    None,
                    None,
                    o("-1"),
                    None,
                    None,
                    o("f"),
                    o("true"),
                    o("a@b.c"),
                ],
            ),
            (
                "",
                vec![
                    None,
                    None,
                    None,
                    None,
                    o("-1"),
                    o(""),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "?q",
                vec![
                    None,
                    None,
                    None,
                    None,
                    o("-1"),
                    o(""),
                    o("q"),
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "#f",
                vec![
                    None,
                    None,
                    None,
                    None,
                    o("-1"),
                    o(""),
                    None,
                    o("f"),
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h/é",
                vec![
                    o("http"),
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o("/é"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http:a",
                vec![
                    o("http"),
                    None,
                    None,
                    None,
                    o("-1"),
                    None,
                    None,
                    None,
                    o("true"),
                    o("a"),
                ],
            ),
            (
                "s+1.x-2:y",
                vec![
                    o("s+1.x-2"),
                    None,
                    None,
                    None,
                    o("-1"),
                    None,
                    None,
                    None,
                    o("true"),
                    o("y"),
                ],
            ),
            (
                "http://u@h/x",
                vec![
                    o("http"),
                    o("u@h"),
                    o("u"),
                    o("h"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "https://h/a/b/../c",
                vec![
                    o("https"),
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o("/a/b/../c"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://[fe80::1%eth0]/x",
                vec![
                    o("http"),
                    o("[fe80::1%eth0]"),
                    None,
                    o("[fe80::1%eth0]"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://a:b:c/x",
                vec![
                    o("http"),
                    o("a:b:c"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h",
                vec![
                    o("http"),
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o(""),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h?q",
                vec![
                    o("http"),
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o(""),
                    o("q"),
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h#",
                vec![
                    o("http"),
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o(""),
                    None,
                    o(""),
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h/x#",
                vec![
                    o("http"),
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o("/x"),
                    None,
                    o(""),
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h/x?",
                vec![
                    o("http"),
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o("/x"),
                    o(""),
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "//h/x",
                vec![
                    None,
                    o("h"),
                    None,
                    o("h"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "///x",
                vec![
                    None,
                    None,
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "a:b:c",
                vec![
                    o("a"),
                    None,
                    None,
                    None,
                    o("-1"),
                    None,
                    None,
                    None,
                    o("true"),
                    o("b:c"),
                ],
            ),
            (
                "http://u@/x",
                vec![
                    o("http"),
                    o("u@"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h:0080/x",
                vec![
                    o("http"),
                    o("h:0080"),
                    None,
                    o("h"),
                    o("80"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://a@b@c/x",
                vec![
                    o("http"),
                    o("a@b@c"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://[2001:db8::1]/x",
                vec![
                    o("http"),
                    o("[2001:db8::1]"),
                    None,
                    o("[2001:db8::1]"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://[::ffff:1.2.3.4]/x",
                vec![
                    o("http"),
                    o("[::ffff:1.2.3.4]"),
                    None,
                    o("[::ffff:1.2.3.4]"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h_ost/x",
                vec![
                    o("http"),
                    o("h_ost"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "1x",
                vec![
                    None,
                    None,
                    None,
                    None,
                    o("-1"),
                    o("1x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://1.2.3.4/x",
                vec![
                    o("http"),
                    o("1.2.3.4"),
                    None,
                    o("1.2.3.4"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://1.2.3.4:8080/x",
                vec![
                    o("http"),
                    o("1.2.3.4:8080"),
                    None,
                    o("1.2.3.4"),
                    o("8080"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://1.2.3.04/x",
                vec![
                    o("http"),
                    o("1.2.3.04"),
                    None,
                    o("1.2.3.04"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://192.168.001.1/x",
                vec![
                    o("http"),
                    o("192.168.001.1"),
                    None,
                    o("192.168.001.1"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://256.1.1.1/x",
                vec![
                    o("http"),
                    o("256.1.1.1"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://1.2.3/x",
                vec![
                    o("http"),
                    o("1.2.3"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://1..3.4/x",
                vec![
                    o("http"),
                    o("1..3.4"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://1.2.3.4x/x",
                vec![
                    o("http"),
                    o("1.2.3.4x"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h-/x",
                vec![
                    o("http"),
                    o("h-"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h-ost/x",
                vec![
                    o("http"),
                    o("h-ost"),
                    None,
                    o("h-ost"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h./x",
                vec![
                    o("http"),
                    o("h."),
                    None,
                    o("h."),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://h:99999999999/x",
                vec![
                    o("http"),
                    o("h:99999999999"),
                    None,
                    None,
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://[0:1:2:3:4:5:1.2.3.4]/x",
                vec![
                    o("http"),
                    o("[0:1:2:3:4:5:1.2.3.4]"),
                    None,
                    o("[0:1:2:3:4:5:1.2.3.4]"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://[1::]/x",
                vec![
                    o("http"),
                    o("[1::]"),
                    None,
                    o("[1::]"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
            (
                "http://[::1.2.3.4]/x",
                vec![
                    o("http"),
                    o("[::1.2.3.4]"),
                    None,
                    o("[::1.2.3.4]"),
                    o("-1"),
                    o("/x"),
                    None,
                    None,
                    o("false"),
                    None,
                ],
            ),
        ] {
            assert_eq!(uri_parts(&uri(input)), want, "{input}");
        }
        // Escapes stay raw in parts (the JDK getters decode; the
        // port has no getters).
        assert_eq!(uri("http://h/a%2Fb").path.as_deref(), Some("/a%2Fb"));
        assert_eq!(
            uri("http://h%20ost/x").authority.as_deref(),
            Some("h%20ost")
        );
        assert!(uri("http://h%20ost/x").host.is_none());
    }

    /// Validity failures with exact reason/index/input (probes
    /// `e-bad-*`, `g-*`).
    #[test]
    fn uri_rejects_invalid() {
        for (input, want) in [
            (
                "http://h/a b",
                "Illegal character in path at index 10: http://h/a b",
            ),
            (
                "http://h/\"a\"",
                "Illegal character in path at index 9: http://h/\"a\"",
            ),
            ("1x:y", "Illegal character in scheme name at index 0: 1x:y"),
            (
                "http://[::1/x",
                "Expected closing bracket for IPv6 address at index 11: http://[::1/x",
            ),
            (
                "http://[gg]/x",
                "Malformed IPv6 address at index 8: http://[gg]/x",
            ),
            (
                "http://h/a%zz",
                "Malformed escape pair at index 10: http://h/a%zz",
            ),
            (
                "http://h/a%2",
                "Malformed escape pair at index 10: http://h/a%2",
            ),
            (
                "http://h/a\u{1}b",
                "Illegal character in path at index 10: http://h/a\u{1}b",
            ),
            (
                "http://h/a\u{7f}b",
                "Illegal character in path at index 10: http://h/a\u{7f}b",
            ),
            (
                "http://h/a\u{80}b",
                "Illegal character in path at index 10: http://h/a\u{80}b",
            ),
            (
                "http://h/a\u{a0}b",
                "Illegal character in path at index 10: http://h/a\u{a0}b",
            ),
            (":", "Expected scheme name at index 0: :"),
            (":x", "Expected scheme name at index 0: :x"),
            (
                "a b:c",
                "Illegal character in scheme name at index 1: a b:c",
            ),
            ("http:", "Expected scheme-specific part at index 5: http:"),
            (
                "http:#f",
                "Expected scheme-specific part at index 5: http:#f",
            ),
            (
                "mailto:",
                "Expected scheme-specific part at index 7: mailto:",
            ),
            ("http://", "Expected authority at index 7: http://"),
            (
                "http://[::1]:8x/x",
                "Illegal character in port number at index 14: http://[::1]:8x/x",
            ),
            (
                "http://a%zz/x",
                "Malformed escape pair at index 8: http://a%zz/x",
            ),
            (
                "http://h/a%",
                "Malformed escape pair at index 10: http://h/a%",
            ),
            (
                "[::1]/x",
                "Illegal character in scheme name at index 0: [::1]/x",
            ),
            (
                "http://[1::2::3]/x",
                "Expected hex digits or IPv4 address at index 13: http://[1::2::3]/x",
            ),
            (
                "http://[12345::]/x",
                "IPv6 hexadecimal digit sequence too long at index 8: http://[12345::]/x",
            ),
            (
                "http://h x/y",
                "Illegal character in authority at index 8: http://h x/y",
            ),
            ("http://[::1%]/x", "scope id expected: http://[::1%]/x"),
            (
                "http://[::12345678901234567890]/x",
                "IPv6 hexadecimal digit sequence too long at index 10: http://[::12345678901234567890]/x",
            ),
            (
                "http://[::1.2.3.12345678901234567890]/x",
                "Malformed IPv4 address at index 16: http://[::1.2.3.12345678901234567890]/x",
            ),
            (
                "http://]/x",
                "Illegal character in hostname at index 7: http://]/x",
            ),
            (
                "http://[::1]x/y",
                "Expected port number at index 12: http://[::1]x/y",
            ),
            (
                "http://[%eth0]/x",
                "Malformed IPv6 address at index 8: http://[%eth0]/x",
            ),
            (
                "http://[0:1:2:3:4:5:6:7:8]/x",
                "IPv6 address too long at index 8: http://[0:1:2:3:4:5:6:7:8]/x",
            ),
            (
                "http://[0:1:2]/x",
                "IPv6 address too short at index 8: http://[0:1:2]/x",
            ),
            (
                "http://[1:2:3:4:5:6:7::8]/x",
                "Malformed IPv6 address at index 8: http://[1:2:3:4:5:6:7::8]/x",
            ),
            (
                "http://[1:2:3:4:5:6:7:8x]/x",
                "Malformed IPv6 address at index 8: http://[1:2:3:4:5:6:7:8x]/x",
            ),
            (
                "http://[1:2:3:4:5:6:7:]/x",
                "Expected digits for an IPv6 address at index 22: http://[1:2:3:4:5:6:7:]/x",
            ),
            (
                "http://[1:23456]/x",
                "IPv6 hexadecimal digit sequence too long at index 10: http://[1:23456]/x",
            ),
            (
                "http://h/a\u{0}b",
                "Illegal character in path at index 10: http://h/a\u{0}b",
            ),
        ] {
            let err = Uri::parse(input).expect_err("expected a URI error");
            assert_eq!(err.to_string(), want, "{input}");
        }
        // `toString` echoes valid input verbatim.
        for s in [
            "",
            "?q",
            "#f",
            "a/b/c",
            "s+1.x-2:y",
            "HTTP://H/X",
            "http://[::1]:8080/x",
            "http://h:99999/x",
            "http://h:/x",
        ] {
            assert_eq!(uri(s).text(), s);
        }
    }

    /// Equality semantics (probes `r-eq-*`, `e-uri-*`, `c-eq-*`).
    #[test]
    fn uri_equals() {
        let eq = |a: &str, b: &str| uri(a).uri_eq(&uri(b));
        for (a, b) in [
            ("http://example.com/x", "http://example.com/x"),
            ("HTTP://example.com/x", "http://example.com/x"),
            ("http://EXAMPLE.com/x", "http://example.com/x"),
            ("http://example.com:80/x", "http://example.com:80/x"),
            ("mailto:a@b.c", "mailto:a@b.c"),
            ("MAILTO:a@b.c", "mailto:a@b.c"),
            ("/a/b", "/a/b"),
            ("http://[::1]/x", "http://[::1]/x"),
            ("http://EXAMPLE/x", "http://example/x"),
            ("http://a;b/x", "http://a;b/x"),
            ("http://[::AB]/x", "http://[::ab]/x"),
            ("http://h/a%2fb", "http://h/a%2Fb"),
            ("http://h/x?a=%2F", "http://h/x?a=%2f"),
            ("http:/x", "http:///x"),
            ("http://h:/x", "http://h/x"),
            ("http://h:8x/x", "http://h:8x/x"),
            ("http://h%2fx/x", "http://h%2Fx/x"),
        ] {
            assert!(eq(a, b), "{a} == {b}");
            assert!(eq(b, a), "{b} == {a}");
        }
        for (a, b) in [
            ("http://example.com/X", "http://example.com/x"),
            ("http://example.com/x?A=1", "http://example.com/x?a=1"),
            ("http://example.com/x#F", "http://example.com/x#f"),
            ("http://U@example.com/x", "http://u@example.com/x"),
            ("http://example.com:80/x", "http://example.com/x"),
            ("http://example.com:81/x", "http://example.com:80/x"),
            ("http://example.com/x", "http://example.com/x/"),
            ("mailto:A@b.c", "mailto:a@b.c"),
            ("/A/b", "/a/b"),
            ("http://h/a%2fb", "http://h/a%2fc"),
            ("http://a;b/x", "http://A;B/x"),
            ("http://h:-1/x", "http://h/x"),
            ("http://h/x", "ftp://h/x"),
            ("http:/x", "http://h:8x/x"),
            ("http://h/x?a=%2F", "http://h/x"),
            ("http://h/a%2Fb", "http://h/a%2Fbc"),
        ] {
            assert!(!eq(a, b), "{a} != {b}");
            assert!(!eq(b, a), "{b} != {a}");
        }
        // Opaque/hierarchical never mix.
        assert!(!eq("mailto:a@b.c", "http://example.com/x"));
        assert!(eq("mailto:a@b.c", "mailto:a@b.c"));
        // Empty-vs-absent components differ (probes `f-empty-*`).
        assert!(!eq("http://@h/x", "http://h/x"));
        assert!(!eq("http://h/x#", "http://h/x"));
        assert!(!eq("http://h/x?", "http://h/x"));
        assert_eq!(uri("http://@h/x").user_info.as_deref(), Some(""));
    }

    /// `hashCode` pins (probes `r-hash-*`, `e-uri-hash-*`,
    /// `c-hash-*`).
    #[test]
    fn uri_hashes() {
        for (input, want) in [
            ("http://example.com/x", 61067039),
            ("HTTP://EXAMPLE.com/x", 61067039),
            ("mailto:a@b.c", 170798086),
            ("/a/b", 1494949),
            ("http://u@example.com:8080/x?y=1#f", -1991072518),
            ("http://h/a%2fb", 1342198872),
            ("http://a;b/x?y#z", 2002189286),
            ("http://u@h:8080/x", 424978148),
            ("mailto:a@b.c#f", 1561868420),
            ("/a?b=c", 293526),
            ("", 0),
            ("http://h:-1/x", 293450965),
            ("http://h:/x", -233510070),
            ("http://h:8x/x", 293451377),
        ] {
            assert_eq!(uri(input).hash_code(), want, "{input}");
        }
    }

    /// `compareTo` pins (probes `r-cmp-*`, `e-cmp-*` — raw ints,
    /// exactly like the JDK; the port's `compare` var takes the
    /// sign).
    #[test]
    fn uri_compares() {
        for (a, b, want) in [
            ("http://example.com/x", "HTTP://EXAMPLE.com/x", 0),
            ("ftp://example.com/x", "http://example.com/x", -2),
            ("http://example.com/a", "http://example.com/b", -1),
            ("http://example.com/x?a=1", "http://example.com/x?a=2", -1),
            ("http://example.com/x#f1", "http://example.com/x#f2", -1),
            ("http://example.com/x", "http://example.com/x#f", -1),
            ("http://example.com:81/x", "http://example.com:80/x", 1),
            ("http://example.com/x", "http://example.com:80/x", -81),
            ("http://a.example/x", "http://b.example/x", -1),
            ("mailto:a@b.c", "mailto:b@b.c", -1),
            ("mailto:a@b.c", "http://example.com/x", 5),
            ("/a", "/b", -1),
            ("http://b/aaa", "http://a/zzz", 1),
            ("http://h/zzz#f-a", "http://h/aaa#f-z", 25),
            ("http://h/zzz?q=a", "http://h/aaa?q=z", 25),
            ("http://h:9/zzz", "http://h:1/aaa", 8),
            ("http://z@a/x", "http://a@z/x", 25),
            ("mailto:zzz#f-a", "mailto:aaa#f-z", 25),
            ("http://h/x", "http://h/x?q", -1),
            ("http://h/x", "http://u@h/x", -1),
            ("http://B@h/x", "http://a@h/x", -31),
            ("http://h/X", "http://h/x", -32),
            ("http:/x", "http://h/x", -1),
            ("http://b;x/y", "http://a;x/y", 1),
            ("http://h/a%2fb", "http://h/a%2Fb", 0),
            ("mailto:a@b.c#f1", "mailto:a@b.c#f2", -1),
            ("mailto:a", "mailto:/x", 1),
            ("mailto:/x", "mailto:a", -1),
            ("http://h/a%2Fb", "http://h/aXb", -51),
            ("http://h/a%2Fb", "http://h/a%3Fb", -1),
            ("http://h/a%2Fb", "http://h/a%2Cb", 3),
            ("//h/x", "http://h/x", -1),
            ("http://h/x", "//h/x", 1),
        ] {
            assert_eq!(uri(a).compare_uri(&uri(b)), want, "{a} <=> {b}");
        }
    }

    fn content_hash(u: &Uri) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut h = DefaultHasher::new();
        u.hash_content(&mut h);
        h.finish()
    }

    /// `hash_content` is `Eq`-consistent with `uri_eq`: equal URIs
    /// hash equally across the opaque, server, and registry shapes
    /// (scheme/host case, escape case, empty-port collapse).
    #[test]
    fn uri_content_hash_matches_eq() {
        for (a, b) in [
            ("http://example.com/x", "HTTP://EXAMPLE.com/x"),
            ("http://h/a%2fb", "http://h/a%2Fb"),
            ("http://h:/x", "http://h/x"),
            ("mailto:a@b.c", "MAILTO:a@b.c"),
            ("http://h:8x/x", "http://h:8x/x"),
            ("http://u@h:8080/a%2Fb?y#f", "http://u@h:8080/a%2fb?y#f"),
        ] {
            assert_eq!(content_hash(&uri(a)), content_hash(&uri(b)), "{a} == {b}");
        }
        assert_ne!(
            content_hash(&uri("http://h/x")),
            content_hash(&uri("mailto:a@b.c"))
        );
        assert_ne!(
            content_hash(&uri("http://h/x")),
            content_hash(&uri("http://h/y"))
        );
        assert_ne!(
            content_hash(&uri("http://h:8x/x")),
            content_hash(&uri("http://h:9x/x"))
        );
    }

    /// Construction ids are unique and increasing (the `#object`
    /// hex in `print.rs`).
    #[test]
    fn uri_ids_are_unique() {
        let a = uri("http://h/x").id();
        let b = uri("http://h/x").id();
        assert_ne!(a, b);
        assert!(b > a);
    }
}
