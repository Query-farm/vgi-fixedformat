//! DATE / TIME / TIMESTAMP display-field codec.
//!
//! A [`crate::layout::FieldKind::DateTime`] field holds its value as fixed-width
//! display text (typically ASCII/EBCDIC digits like `YYYYMMDD`). This module
//! parses those bytes into a temporal [`Value`] using a strftime-style `format`
//! pattern via [`chrono`], and formats one back into bytes — the exact inverse.
//! Pure compute (no Arrow/VGI); the bytes have already been transcoded to ASCII
//! by the caller (`decode`/`encode` handle the EBCDIC ↔ ASCII step).
//!
//! 2-digit years: a `%y` token is handled by chrono's standard window — values
//! `00..=68` map to `2000..=2068` and `69..=99` to `1969..=1999`. Use `%Y` when
//! you need the full four-digit year.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::layout::DateTimeKind;
use crate::value::Value;
use crate::{Error, Result};

/// The Unix epoch as a `NaiveDate` (the zero point for [`Value::Date`]).
fn epoch_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a valid date")
}

/// Parse the ASCII `bytes` of a `DateTime` field as `kind` using `format`,
/// producing a [`Value::Date`] / [`Value::Time`] / [`Value::Timestamp`]. `field`
/// names the field for error messages.
pub fn parse(kind: DateTimeKind, format: &str, bytes: &[u8], field: &str) -> Result<Value> {
    let text = String::from_utf8_lossy(bytes);
    let s = text.trim();
    let bad = |what: &str| {
        Error(format!(
            "field {field}: cannot parse {what} {s:?} (bytes {bytes:?}) with format {format:?}"
        ))
    };
    Ok(match kind {
        DateTimeKind::Date => {
            let d = NaiveDate::parse_from_str(s, format).map_err(|_| bad("date"))?;
            Value::Date((d - epoch_date()).num_days() as i32)
        }
        DateTimeKind::Time => {
            let t = NaiveTime::parse_from_str(s, format).map_err(|_| bad("time"))?;
            Value::Time(time_to_micros(t))
        }
        DateTimeKind::Timestamp => {
            let dt = NaiveDateTime::parse_from_str(s, format).map_err(|_| bad("timestamp"))?;
            Value::Timestamp(dt.and_utc().timestamp_micros())
        }
    })
}

/// Format a temporal [`Value`] back into `width` bytes using `format`. Errors if
/// the rendered text is longer than `width`; pads shorter output with trailing
/// spaces (which `parse` trims). `field` names the field for error messages.
pub fn format(value: &Value, format: &str, width: usize, field: &str) -> Result<Vec<u8>> {
    let rendered = match value {
        Value::Date(days) => {
            let d = epoch_date()
                .checked_add_signed(Duration::days(*days as i64))
                .ok_or_else(|| Error(format!("field {field}: date {days} out of range")))?;
            d.format(format).to_string()
        }
        Value::Time(micros) => time_from_micros(*micros, field)?.format(format).to_string(),
        Value::Timestamp(micros) => DateTime::from_timestamp_micros(*micros)
            .ok_or_else(|| Error(format!("field {field}: timestamp {micros} out of range")))?
            .naive_utc()
            .format(format)
            .to_string(),
        Value::Null => String::new(),
        other => {
            return Err(Error(format!(
                "field {field}: expected a date/time value, got {other:?}"
            )))
        }
    };
    let bytes = rendered.into_bytes();
    if bytes.len() > width {
        return Err(Error(format!(
            "field {field}: formatted value of {} bytes does not fit in field width {width}",
            bytes.len()
        )));
    }
    let mut out = vec![b' '; width];
    out[..bytes.len()].copy_from_slice(&bytes);
    Ok(out)
}

fn time_to_micros(t: NaiveTime) -> i64 {
    t.num_seconds_from_midnight() as i64 * 1_000_000 + (t.nanosecond() as i64) / 1_000
}

fn time_from_micros(micros: i64, field: &str) -> Result<NaiveTime> {
    let secs = micros.div_euclid(1_000_000);
    let nanos = micros.rem_euclid(1_000_000) * 1_000;
    NaiveTime::from_num_seconds_from_midnight_opt(secs as u32, nanos as u32)
        .ok_or_else(|| Error(format!("field {field}: time {micros} out of range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_round_trip() {
        let v = parse(DateTimeKind::Date, "%Y%m%d", b"20240131", "d").unwrap();
        // 2024-01-31 is 19753 days after the epoch.
        assert_eq!(v, Value::Date(19753));
        let bytes = format(&v, "%Y%m%d", 8, "d").unwrap();
        assert_eq!(&bytes, b"20240131");
    }

    #[test]
    fn date_with_separators_round_trip() {
        let v = parse(DateTimeKind::Date, "%Y-%m-%d", b"2024-01-31", "d").unwrap();
        assert_eq!(v, Value::Date(19753));
        assert_eq!(&format(&v, "%Y-%m-%d", 10, "d").unwrap(), b"2024-01-31");
    }

    #[test]
    fn timestamp_round_trip() {
        let v = parse(
            DateTimeKind::Timestamp,
            "%Y%m%d%H%M%S",
            b"20240131123045",
            "ts",
        )
        .unwrap();
        match v {
            Value::Timestamp(_) => {}
            other => panic!("expected timestamp, got {other:?}"),
        }
        let bytes = format(&v, "%Y%m%d%H%M%S", 14, "ts").unwrap();
        assert_eq!(&bytes, b"20240131123045");
    }

    #[test]
    fn time_round_trip() {
        let v = parse(DateTimeKind::Time, "%H%M%S", b"123045", "t").unwrap();
        assert_eq!(
            v,
            Value::Time(((12 * 3600 + 30 * 60 + 45) as i64) * 1_000_000)
        );
        assert_eq!(&format(&v, "%H%M%S", 6, "t").unwrap(), b"123045");
    }

    #[test]
    fn bad_date_errors() {
        let err = parse(DateTimeKind::Date, "%Y%m%d", b"2024XX31", "d").unwrap_err();
        assert!(err.0.contains("field d"), "{}", err.0);
        assert!(err.0.contains("cannot parse date"), "{}", err.0);
    }

    #[test]
    fn format_overflow_errors() {
        let v = Value::Date(19753);
        let err = format(&v, "%Y-%m-%d", 8, "d").unwrap_err();
        assert!(err.0.contains("does not fit"), "{}", err.0);
    }

    #[test]
    fn short_format_is_space_padded() {
        let v = Value::Date(19753);
        let bytes = format(&v, "%Y%m%d", 10, "d").unwrap();
        assert_eq!(&bytes, b"20240131  ");
    }
}
