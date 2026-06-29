//! COBOL **edited** (PICTURE-editing) numeric fields — the "print image" /
//! report formats like `ZZ,ZZ9.99`, `$$$,$$9.99`, `9(5)CR`, `***1.50`, or
//! `---,--9`. These carry a number dressed up with insertion characters (commas,
//! the decimal point, slashes, spaces, inserted zeros), zero-suppression
//! (`Z`/`*`), currency/sign symbols (`$`/`+`/`-`), and `CR`/`DB` credit markers.
//!
//! This module turns those bytes back into the underlying numeric value (decode)
//! and best-effort renders a value back into an edited width (encode). The
//! [`FieldKind::Edited`](crate::layout::FieldKind::Edited) carries the field's
//! `precision`, `scale`, `signed` flag, and the expanded edit `mask` (the
//! width-bearing characters of the PIC, e.g. `"ZZ,ZZ9.99"`).
//!
//! **Decode** is mask-agnostic: strip every byte that is not a digit, read the
//! sign from a `CR`/`DB` marker or a leading/trailing `+`/`-`, and parse the
//! remaining digits as an unscaled integer at the field's `scale`. **Encode**
//! walks the mask and renders the supported (non-floating) editing characters;
//! floating currency/sign runs (`$$`, `++`, `--`) are not supported for write
//! and produce a clear error.

use crate::{Error, Result};

/// Decode an edited numeric field's (already ASCII-transcoded) bytes into an
/// unscaled integer at `scale`. Non-digit characters are insertion symbols and
/// are ignored; the sign comes from a trailing `CR`/`DB` or a leading/trailing
/// `+`/`-`. `scale` is metadata for the caller's `DECIMAL(p, s)` — the decimal
/// point in the bytes is just another non-digit that gets stripped.
pub fn decode(bytes: &[u8], _scale: u8, signed: bool) -> Result<i128> {
    let s = String::from_utf8_lossy(bytes);
    let negative = signed && is_negative(&s);
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    let mag: i128 = if digits.is_empty() {
        0
    } else {
        digits
            .parse()
            .map_err(|_| Error(format!("invalid edited numeric field: {digits:?}")))?
    };
    Ok(if negative { -mag } else { mag })
}

/// Whether the edited image denotes a negative value: a `CR`/`DB` marker, or a
/// leading/trailing `-` (a leading/trailing `+` is an explicit positive).
fn is_negative(s: &str) -> bool {
    let up = s.trim().to_ascii_uppercase();
    if up.ends_with("CR") || up.ends_with("DB") {
        return true;
    }
    up.starts_with('-') || up.ends_with('-')
}

/// Render an unscaled value at `scale` into the edited `mask`, returning exactly
/// `mask.len()` bytes. Supports the fixed (non-floating) editing characters:
/// digit positions `9`/`Z`/`*`, insertions `,`/`.`/`/`/`B`/`0`, a fixed leading
/// `$`, leading/trailing `+`/`-`, and trailing `CR`/`DB`. Floating currency/sign
/// runs (`$$`, `++`, `--`) are rejected — use a plain numeric PIC for write.
pub fn encode(unscaled: i128, _scale: u8, mask: &str) -> Result<Vec<u8>> {
    if floating(mask, '$') || floating(mask, '+') || floating(mask, '-') {
        return Err(Error(format!(
            "edited PIC {mask:?} uses a floating currency/sign string, which is not supported \
             for write — use a plain numeric PIC (e.g. S9(n)V99) for output"
        )));
    }

    let chars: Vec<char> = mask.chars().collect();
    let total_digits = chars.iter().filter(|c| is_digit_pos(**c)).count();
    let negative = unscaled < 0;
    let mag = unscaled.unsigned_abs().to_string();
    if mag.len() > total_digits {
        return Err(Error(format!(
            "value {unscaled} does not fit the {total_digits} digit position(s) of edited PIC \
             {mask:?}"
        )));
    }
    // Left-pad the magnitude to fill every digit position.
    let mag_digits: Vec<u8> =
        format!("{}{}", "0".repeat(total_digits - mag.len()), mag).into_bytes();
    let pad = if mask.contains('*') { b'*' } else { b' ' };

    let mut out: Vec<u8> = Vec::with_capacity(chars.len());
    let mut di = 0usize; // next magnitude digit
    let mut suppressing = true; // leading-zero suppression active (integer part)
    let mut k = 0usize;
    while k < chars.len() {
        match chars[k] {
            '9' => {
                out.push(mag_digits[di]);
                di += 1;
                suppressing = false;
            }
            c @ ('Z' | '*') => {
                let d = mag_digits[di];
                di += 1;
                if suppressing && d == b'0' {
                    out.push(if c == 'Z' { b' ' } else { b'*' });
                } else {
                    suppressing = false;
                    out.push(d);
                }
            }
            ',' => out.push(if suppressing { pad } else { b',' }),
            '.' => {
                out.push(b'.');
                suppressing = false;
            }
            '/' => out.push(b'/'),
            'B' => out.push(b' '),
            '0' => out.push(b'0'),
            '$' => out.push(b'$'),
            '+' => out.push(if negative { b'-' } else { b'+' }),
            '-' => out.push(if negative { b'-' } else { b' ' }),
            'C' => {
                // CR credit marker (two bytes): shown only when negative.
                let (a, b) = if negative { (b'C', b'R') } else { (b' ', b' ') };
                out.push(a);
                out.push(b);
                k += 1; // consume the paired 'R'
            }
            'D' => {
                // DB debit marker (two bytes): shown only when negative.
                let (a, b) = if negative { (b'D', b'B') } else { (b' ', b' ') };
                out.push(a);
                out.push(b);
                k += 1; // consume the paired 'B'
            }
            other => {
                return Err(Error(format!(
                    "unexpected character {other:?} in edited PIC mask {mask:?}"
                )))
            }
        }
        k += 1;
    }
    Ok(out)
}

/// A digit-bearing mask position (`9`/`Z`/`*`); insertion and sign characters are
/// not counted.
fn is_digit_pos(c: char) -> bool {
    matches!(c, '9' | 'Z' | '*')
}

/// Whether `mask` contains a *floating* run of `sym` (two or more occurrences) —
/// e.g. `$$$`, `+++`, `---`. A single occurrence is a fixed currency/sign symbol.
fn floating(mask: &str, sym: char) -> bool {
    mask.chars().filter(|&c| c == sym).count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_currency_and_commas() {
        // "$1,234.56" → 123456 at scale 2.
        assert_eq!(decode(b"$1,234.56", 2, false).unwrap(), 123456);
    }

    #[test]
    fn decode_zero_suppressed() {
        // ZZ,ZZ9.99 over "   123.45" → 12345 at scale 2.
        assert_eq!(decode(b"   123.45", 2, false).unwrap(), 12345);
    }

    #[test]
    fn decode_check_protection() {
        // ***1.50 → 150 at scale 2.
        assert_eq!(decode(b"***1.50", 2, false).unwrap(), 150);
    }

    #[test]
    fn decode_cr_is_negative() {
        // 9(5)CR over "00123CR" → -123.
        assert_eq!(decode(b"00123CR", 0, true).unwrap(), -123);
        assert_eq!(decode(b"00123  ", 0, true).unwrap(), 123);
    }

    #[test]
    fn decode_db_is_negative() {
        assert_eq!(decode(b"00123DB", 0, true).unwrap(), -123);
    }

    #[test]
    fn decode_leading_and_trailing_sign() {
        assert_eq!(decode(b"-123", 0, true).unwrap(), -123);
        assert_eq!(decode(b"+123", 0, true).unwrap(), 123);
        assert_eq!(decode(b"123-", 0, true).unwrap(), -123);
    }

    #[test]
    fn decode_ignores_sign_when_unsigned() {
        // An unsigned edited field never goes negative.
        assert_eq!(decode(b"123-", 0, false).unwrap(), 123);
    }

    #[test]
    fn encode_zero_suppressed_round_trips() {
        // ZZ,ZZ9.99 with 12345 (123.45) → "   123.45".
        let mask = "ZZ,ZZ9.99";
        let bytes = encode(12345, 2, mask).unwrap();
        assert_eq!(bytes.len(), mask.len());
        assert_eq!(&bytes, b"   123.45");
        assert_eq!(decode(&bytes, 2, false).unwrap(), 12345);
    }

    #[test]
    fn encode_check_protection_round_trips() {
        // **,**9.99 with 150 (1.50) → "*****1.50".
        let mask = "**,**9.99";
        let bytes = encode(150, 2, mask).unwrap();
        assert_eq!(&bytes, b"*****1.50");
        assert_eq!(decode(&bytes, 2, false).unwrap(), 150);
    }

    #[test]
    fn encode_cr_round_trips() {
        // 9(5)CR — mask "99999CR".
        let mask = "99999CR";
        let neg = encode(-123, 0, mask).unwrap();
        assert_eq!(&neg, b"00123CR");
        assert_eq!(decode(&neg, 0, true).unwrap(), -123);
        let pos = encode(123, 0, mask).unwrap();
        assert_eq!(&pos, b"00123  ");
        assert_eq!(decode(&pos, 0, true).unwrap(), 123);
    }

    #[test]
    fn encode_trailing_minus_round_trips() {
        let mask = "ZZZ9-";
        let neg = encode(-42, 0, mask).unwrap();
        assert_eq!(&neg, b"  42-");
        assert_eq!(decode(&neg, 0, true).unwrap(), -42);
        let pos = encode(42, 0, mask).unwrap();
        assert_eq!(&pos, b"  42 ");
        assert_eq!(decode(&pos, 0, true).unwrap(), 42);
    }

    #[test]
    fn encode_fixed_currency_round_trips() {
        // A single (fixed) '$' is allowed.
        let mask = "$9,999.99";
        let bytes = encode(123456, 2, mask).unwrap();
        assert_eq!(&bytes, b"$1,234.56");
        assert_eq!(decode(&bytes, 2, false).unwrap(), 123456);
    }

    #[test]
    fn encode_rejects_floating() {
        assert!(encode(123456, 2, "$$$,$$9.99").is_err());
        assert!(encode(-123, 0, "---,--9").is_err());
    }

    #[test]
    fn encode_overflow_errors() {
        assert!(encode(123456, 0, "999").is_err());
    }
}
