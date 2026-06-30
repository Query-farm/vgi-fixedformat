//! Perl-`unpack` / Python-`struct`-style template parser → [`Layout`].
//!
//! Whitespace-separated tokens, each optionally prefixed `name:`. A token is
//! either a byte-order control (`<` `>` `!` `=` `@`) that sets the default order
//! for following tokens, a format code with an optional count, or a COBOL-ish
//! display PIC token (`9(5)`, `S9(7)V99`, `X(10)`) handled by [`crate::copybook::parse_pic`].
//!
//! Format codes (count in `(n)` or as trailing digits):
//!
//! | code        | meaning                              | count is |
//! |-------------|--------------------------------------|----------|
//! | `A`/`a`/`Z` | string (space / null pad / null-term)| width    |
//! | `c`/`C`     | int8 / uint8                         | repeat   |
//! | `s`/`S`     | int16 / uint16                       | repeat   |
//! | `l`/`L` `i`/`I` | int32 / uint32                   | repeat   |
//! | `q`/`Q`     | int64 / uint64                       | repeat   |
//! | `n`/`N`     | uint16 / uint32, big-endian          | repeat   |
//! | `v`/`V`     | uint16 / uint32, little-endian       | repeat   |
//! | `e`/`f`/`d` | float16 / float32 / float64          | repeat   |
//! | `H`/`h`     | hex string (high / low nibble first) | width    |
//! | `?`         | boolean byte                         | repeat   |
//! | `x`         | pad byte(s)                          | width    |

use crate::copybook::{self, Usage};
use crate::layout::{Endian, Field, FieldKind, Justify, Layout};
use crate::{Error, Result};

const NATIVE: Endian = if cfg!(target_endian = "little") {
    Endian::Little
} else {
    Endian::Big
};

/// Parse a template string into a [`Layout`].
///
/// Beyond the flat tokens, two Perl-`unpack` constructs are supported:
/// - **`(...)` groups** → a `STRUCT`; a trailing count repeats the group → a
///   `LIST` of `STRUCT`. e.g. `lines:(sku:A10 qty:9(5))3`.
/// - **`code/(...)` count-prefix** (Perl's `/`) → a count field of type `code`
///   followed by *that many* group occurrences → a count-driven `LIST` (the
///   template form of `OCCURS … DEPENDING ON`). e.g. `lines:N/(sku:A10 qty:9(5))`.
pub fn parse(src: &str) -> Result<Layout> {
    let tokens = tokenize(src)?;
    let (fields, _width) = parse_tokens(&tokens)?;
    Layout::from_fields(fields)
}

/// Split a template into tokens on whitespace, but **not** inside `(...)` groups
/// (so `(sku:A10 qty:9(5))` stays one token while `9(5)`'s parens — which hold no
/// space — are unaffected).
fn tokenize(src: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in src.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(Error("template: unbalanced ')'".into()));
                }
                cur.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if depth != 0 {
        return Err(Error("template: unbalanced '('".into()));
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    Ok(tokens)
}

/// Split an optional `name:` prefix off a token. The `:` only counts as a name
/// separator when it precedes any `(` or `/`, so a `:` inside a group body (e.g.
/// the unnamed `N/(sku:A10 …)`) isn't mistaken for a field name.
fn split_name(token: &str) -> (Option<String>, &str) {
    let colon = token.find(':');
    let before = |limit: Option<usize>| limit.is_none_or(|p| colon.unwrap() < p);
    match colon {
        Some(c) if before(token.find('(')) && before(token.find('/')) => {
            (Some(token[..c].to_string()), &token[c + 1..])
        }
        _ => (None, token),
    }
}

/// A `(...)` group body → its inner template string + an optional trailing repeat
/// count. `body` must start with `(`.
fn strip_group(body: &str) -> Result<(String, Option<usize>)> {
    let chars: Vec<char> = body.chars().collect();
    let mut depth = 0i32;
    let mut close = None;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or_else(|| Error(format!("template: unbalanced group {body:?}")))?;
    let inner: String = chars[1..close].iter().collect();
    let trailing: String = chars[close + 1..].iter().collect();
    let count = parse_count(&trailing.chars().collect::<Vec<_>>())?;
    Ok((inner, count))
}

/// Parse a token list into fields (offsets relative to this list's start) and the
/// total reserved width. Recurses for `(...)` groups.
fn parse_tokens(tokens: &[String]) -> Result<(Vec<Field>, usize)> {
    let mut fields = Vec::new();
    let mut offset = 0usize;
    let mut order = Endian::Big; // default network order; `<`/`=` change it
    let mut auto = 0usize;
    let name_or_auto = |opt: Option<String>, auto: &mut usize| {
        opt.unwrap_or_else(|| {
            *auto += 1;
            format!("field_{auto}")
        })
    };

    for token in tokens {
        if let Some(o) = order_control(token) {
            order = o;
            continue;
        }
        let (name_opt, body) = split_name(token);
        if body.is_empty() {
            return Err(Error(format!("template: empty token {token:?}")));
        }

        // `code/(...)` — a count field then that many group occurrences (Perl `/`).
        if let Some(idx) = body.find("/(") {
            let (code, group) = body.split_at(idx);
            let (cnt_kind, cnt_width, cnt_occurs) = parse_body(code, order)?;
            if cnt_occurs.is_some()
                || !matches!(cnt_kind, FieldKind::Binary { .. } | FieldKind::Int { .. })
            {
                return Err(Error(format!(
                    "template: the count before '/' must be a single integer code, got {code:?}"
                )));
            }
            let (inner_src, trailing) = strip_group(&group[1..])?; // skip '/'
            if trailing.is_some() {
                return Err(Error(
                    "template: a count-prefixed group 'code/(...)' takes no trailing repeat count"
                        .into(),
                ));
            }
            let (children, gw) = parse_tokens(&tokenize(&inner_src)?)?;
            let base = name_or_auto(name_opt, &mut auto);
            let count_name = format!("{base}_count");
            // The count field is visible (so the row carries the controller).
            fields.push(Field {
                name: count_name.clone(),
                offset,
                width: cnt_width,
                kind: cnt_kind,
                occurs: None,
                depending_on: None,
                redefines: None,
            });
            offset += cnt_width;
            // The table reserves zero static footprint (variable length).
            fields.push(Field {
                name: base,
                offset,
                width: gw,
                kind: FieldKind::Group(children),
                occurs: None,
                depending_on: Some(count_name),
                redefines: None,
            });
            continue;
        }

        // `(...)` group, with an optional trailing repeat count.
        if body.starts_with('(') {
            let (inner_src, count) = strip_group(body)?;
            let (children, gw) = parse_tokens(&tokenize(&inner_src)?)?;
            fields.push(Field {
                name: name_or_auto(name_opt, &mut auto),
                offset,
                width: gw,
                kind: FieldKind::Group(children),
                occurs: count,
                depending_on: None,
                redefines: None,
            });
            offset += gw * count.unwrap_or(1);
            continue;
        }

        // Plain scalar code.
        let (kind, width, occurs) = parse_body(body, order)?;
        let total = width * occurs.unwrap_or(1);
        fields.push(Field {
            name: name_or_auto(name_opt, &mut auto),
            offset,
            width,
            kind,
            occurs,
            depending_on: None,
            redefines: None,
        });
        offset += total;
    }

    Ok((fields, offset))
}

fn order_control(token: &str) -> Option<Endian> {
    match token {
        "<" => Some(Endian::Little),
        ">" | "!" => Some(Endian::Big),
        "=" | "@" => Some(NATIVE),
        _ => None,
    }
}

/// Parse a token body into `(kind, width, occurs)`.
fn parse_body(body: &str, order: Endian) -> Result<(FieldKind, usize, Option<usize>)> {
    let chars: Vec<char> = body.chars().collect();
    let code = chars[0];

    // PIC-looking display tokens delegate to the shared copybook PIC parser.
    if code == '9' || code == 'X' || (code == 'S' && chars.get(1) == Some(&'9')) {
        let (kind, width) = copybook::parse_pic(body, Usage::Display, None)?;
        return Ok((kind, width, None));
    }

    // Otherwise: a single-letter code, an optional count, an optional order
    // suffix (`<`/`>`).
    let mut rest = &chars[1..];
    let mut local_order = order;
    if let Some(&last) = rest.last() {
        if last == '<' {
            local_order = Endian::Little;
            rest = &rest[..rest.len() - 1];
        } else if last == '>' {
            local_order = Endian::Big;
            rest = &rest[..rest.len() - 1];
        }
    }
    let count = parse_count(rest)?;

    let width_kind = |w: usize, k: FieldKind| (k, w, None);
    let counted = |elem_width: usize, k: FieldKind| {
        // count = repeat → LIST when > 1.
        if let Some(n) = count {
            (k, elem_width, Some(n))
        } else {
            (k, elem_width, None)
        }
    };

    Ok(match code {
        'A' => width_kind(
            count.unwrap_or(1),
            FieldKind::Text {
                justify: Justify::Left,
                trim: true,
                pad: b' ',
            },
        ),
        'a' => width_kind(
            count.unwrap_or(1),
            FieldKind::Text {
                justify: Justify::Left,
                trim: true,
                pad: 0,
            },
        ),
        'Z' => width_kind(
            count.unwrap_or(1),
            FieldKind::Text {
                justify: Justify::Left,
                trim: true,
                pad: 0,
            },
        ),
        'H' => width_kind(count.unwrap_or(1), FieldKind::Hex { order: Endian::Big }),
        'h' => width_kind(
            count.unwrap_or(1),
            FieldKind::Hex {
                order: Endian::Little,
            },
        ),
        'x' => width_kind(count.unwrap_or(1), FieldKind::Pad { pad: 0 }),
        'c' => counted(
            1,
            FieldKind::Binary {
                endian: local_order,
                signed: true,
            },
        ),
        'C' => counted(
            1,
            FieldKind::Binary {
                endian: local_order,
                signed: false,
            },
        ),
        's' => counted(
            2,
            FieldKind::Binary {
                endian: local_order,
                signed: true,
            },
        ),
        'S' => counted(
            2,
            FieldKind::Binary {
                endian: local_order,
                signed: false,
            },
        ),
        'l' | 'i' => counted(
            4,
            FieldKind::Binary {
                endian: local_order,
                signed: true,
            },
        ),
        'L' | 'I' => counted(
            4,
            FieldKind::Binary {
                endian: local_order,
                signed: false,
            },
        ),
        'q' => counted(
            8,
            FieldKind::Binary {
                endian: local_order,
                signed: true,
            },
        ),
        'Q' => counted(
            8,
            FieldKind::Binary {
                endian: local_order,
                signed: false,
            },
        ),
        'n' => counted(
            2,
            FieldKind::Binary {
                endian: Endian::Big,
                signed: false,
            },
        ),
        'N' => counted(
            4,
            FieldKind::Binary {
                endian: Endian::Big,
                signed: false,
            },
        ),
        'v' => counted(
            2,
            FieldKind::Binary {
                endian: Endian::Little,
                signed: false,
            },
        ),
        'V' => counted(
            4,
            FieldKind::Binary {
                endian: Endian::Little,
                signed: false,
            },
        ),
        'e' => counted(
            2,
            FieldKind::Float {
                bits: 16,
                endian: local_order,
            },
        ),
        'f' => counted(
            4,
            FieldKind::Float {
                bits: 32,
                endian: local_order,
            },
        ),
        'd' => counted(
            8,
            FieldKind::Float {
                bits: 64,
                endian: local_order,
            },
        ),
        '?' => counted(1, FieldKind::Bool),
        other => return Err(Error(format!("unknown template code {other:?}"))),
    })
}

/// Parse a count: either `(n)` or bare trailing digits. Empty ⇒ None.
fn parse_count(rest: &[char]) -> Result<Option<usize>> {
    if rest.is_empty() {
        return Ok(None);
    }
    let s: String = rest.iter().collect();
    let inner = if let Some(stripped) = s.strip_prefix('(') {
        stripped
            .strip_suffix(')')
            .ok_or_else(|| Error(format!("unbalanced count parentheses in {s:?}")))?
    } else {
        &s
    };
    let n: usize = inner
        .trim()
        .parse()
        .map_err(|_| Error(format!("invalid count {inner:?}")))?;
    Ok(Some(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_record;
    use crate::value::Value;
    use crate::Encoding;

    #[test]
    fn parses_string_and_display_int() {
        let layout = parse("name:A10 qty:9(5)").unwrap();
        assert_eq!(layout.record_len, 15);
        assert_eq!(layout.fields[0].name, "name");
        assert_eq!(layout.fields[1].name, "qty");
        let out = decode_record(&layout, b"JOHN      00042", Encoding::Ascii).unwrap();
        assert_eq!(out[1].1, Value::Int(42));
    }

    #[test]
    fn auto_names_fields() {
        let layout = parse("A10 9(5)").unwrap();
        assert_eq!(layout.fields[0].name, "field_1");
        assert_eq!(layout.fields[1].name, "field_2");
    }

    #[test]
    fn binary_codes_and_order() {
        let layout = parse("a:s< b:l> c:N").unwrap();
        assert_eq!(layout.record_len, 2 + 4 + 4);
        match layout.fields[0].kind {
            FieldKind::Binary { endian, signed } => {
                assert_eq!(endian, Endian::Little);
                assert!(signed);
            }
            _ => panic!(),
        }
        match layout.fields[2].kind {
            FieldKind::Binary { endian, signed } => {
                assert_eq!(endian, Endian::Big);
                assert!(!signed);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn order_control_token_sets_default() {
        let layout = parse("< s S").unwrap();
        for f in &layout.fields {
            match f.kind {
                FieldKind::Binary { endian, .. } => assert_eq!(endian, Endian::Little),
                _ => panic!(),
            }
        }
    }

    #[test]
    fn repeat_count_makes_list() {
        let layout = parse("vals:s(3)").unwrap();
        assert_eq!(layout.fields[0].occurs, Some(3));
        assert_eq!(layout.record_len, 6);
    }

    #[test]
    fn float_and_pad_and_hex() {
        let layout = parse("f:d g:H4 x(2) b:?").unwrap();
        assert_eq!(layout.record_len, 8 + 4 + 2 + 1);
        assert!(matches!(layout.fields[1].kind, FieldKind::Hex { .. }));
        assert!(matches!(layout.fields[2].kind, FieldKind::Pad { .. }));
        assert!(matches!(layout.fields[3].kind, FieldKind::Bool));
    }

    #[test]
    fn edited_pic_token_decodes() {
        // A `9`-leading edited PIC token decodes to a DECIMAL value. (Edited PICs
        // beginning with `Z`/`$`/`*` collide with template string codes, so a
        // template edited field must start with a `9`/`S9` digit.)
        let layout = parse("amt:9(4).99 flag:9(5)CR").unwrap();
        assert!(matches!(
            layout.fields[0].kind,
            FieldKind::Edited { scale: 2, .. }
        ));
        assert_eq!(layout.fields[0].width, 7); // 9999.99
        assert!(matches!(
            layout.fields[1].kind,
            FieldKind::Edited {
                signed: true,
                scale: 0,
                ..
            }
        ));
        let out = decode_record(&layout, b"0012.5000123CR", Encoding::Ascii).unwrap();
        assert_eq!(
            out[0].1,
            Value::Decimal {
                unscaled: 1250,
                scale: 2
            }
        );
        assert_eq!(
            out[1].1,
            Value::Decimal {
                unscaled: -123,
                scale: 0
            }
        );
    }

    #[test]
    fn group_produces_struct() {
        // `(...)` → a STRUCT field.
        let layout = parse("hdr:A2 item:(sku:A3 qty:s)").unwrap();
        assert_eq!(layout.fields.len(), 2);
        assert_eq!(layout.record_len, 2 + 3 + 2);
        let children = match &layout.fields[1].kind {
            FieldKind::Group(c) => c,
            _ => panic!("expected group"),
        };
        assert_eq!(children.len(), 2);
        let out = decode_record(&layout, b"XYABC\x00\x07", Encoding::Ascii).unwrap();
        match &out[1].1 {
            Value::Struct(pairs) => {
                assert_eq!(pairs[0].1, Value::Text("ABC".into()));
                assert_eq!(pairs[1].1, Value::Int(7));
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn repeated_group_is_list_of_struct() {
        // `(...)N` → a LIST of N STRUCTs (Perl `(A3 C)2`).
        let layout = parse("items:(sku:A3 qty:C)2").unwrap();
        assert_eq!(layout.fields[0].occurs, Some(2));
        assert_eq!(layout.record_len, (3 + 1) * 2);
        let out = decode_record(&layout, b"ABC\x05DEF\x09", Encoding::Ascii).unwrap();
        match &out[0].1 {
            Value::List(items) => {
                assert_eq!(items.len(), 2);
                match &items[1] {
                    Value::Struct(p) => {
                        assert_eq!(p[0].1, Value::Text("DEF".into()));
                        assert_eq!(p[1].1, Value::Int(9));
                    }
                    _ => panic!(),
                }
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn count_prefixed_group_is_a_depending_list() {
        // `code/(...)` (Perl `/`) → a count field then that many group occurrences.
        let layout = parse("lines:C/(sku:A3 qty:C)").unwrap();
        assert!(layout.variable, "a count-prefixed group is variable-length");
        assert_eq!(layout.fields[0].name, "lines_count");
        assert_eq!(layout.fields[1].name, "lines");
        assert_eq!(
            layout.fields[1].depending_on.as_deref(),
            Some("lines_count")
        );
        // count = 2, then two (sku A3, qty C) groups.
        let out = decode_record(&layout, b"\x02ABC\x05DEF\x09", Encoding::Ascii).unwrap();
        assert_eq!(out[0].1, Value::Int(2));
        match &out[1].1 {
            Value::List(items) => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    Value::Struct(p) => assert_eq!(p[1].1, Value::Int(5)),
                    _ => panic!(),
                }
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn count_before_slash_must_be_integer() {
        assert!(parse("x:A3/(a:C)").is_err());
    }

    #[test]
    fn decodes_against_struct_pack_bytes() {
        // Python: struct.pack('>hIf', -2, 7, 1.5) == b'\xff\xfe\x00\x00\x00\x07?\xc0\x00\x00'
        let layout = parse("a:s b:I c:f").unwrap();
        let bytes = [0xff, 0xfe, 0x00, 0x00, 0x00, 0x07, 0x3f, 0xc0, 0x00, 0x00];
        let out = decode_record(&layout, &bytes, Encoding::Ascii).unwrap();
        assert_eq!(out[0].1, Value::Int(-2));
        assert_eq!(out[1].1, Value::Int(7));
        assert_eq!(out[2].1, Value::Float(1.5));
    }
}
