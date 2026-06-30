# vgi-fixedformat

Read and write **fixed-width / flat-file / mainframe** data in DuckDB with SQL —
the equivalent of Perl `unpack()` / Python `struct`, plus COBOL copybooks
(COMP-3, zoned decimal, EBCDIC, `OCCURS` / `OCCURS … DEPENDING ON`, `REDEFINES`).
Nested records become `STRUCT`s, repeating groups become `LIST`s, and
`describe_fixed` shows you exactly how any spec resolves before you run it.

It runs as a [VGI worker](https://query.farm): a small standalone binary that
DuckDB launches and talks to over Apache Arrow. You `ATTACH` it and call its
functions like any other.

```sql
ATTACH 'fixed' (TYPE vgi, LOCATION './target/release/fixedformat-worker');
SET search_path = 'fixed.main';

SELECT unpack_fixed('JOHN      00042', 'name:A10 qty:9(5)');
-- {'name': JOHN, 'qty': 42}
```

---

## Quick start

**1. Get the worker binary.** Either download a prebuilt archive from the
[Releases page](https://github.com/Query-farm/vgi-fixedformat/releases) for your
platform (`vgi-fixedformat-<version>-<platform>.tar.gz`, where `<platform>` is
one of `linux_amd64`, `linux_arm64`, `osx_amd64`, `osx_arm64`, `windows_amd64`)
and unpack the `fixedformat-worker` executable…

```sh
# Replace <version> with the release tag, e.g. v0.4.0, and pick your platform.
tar -xzf vgi-fixedformat-<version>-osx_arm64.tar.gz   # → fixedformat-worker
```

…or build it from source (needs Rust 1.90+):

```sh
cargo build --release          # produces target/release/fixedformat-worker
```

Each release archive is accompanied by a SHA256 checksum, a keyless `cosign`
signature (`.cosign.bundle`), and a SLSA build-provenance attestation — see the
[release-tooling docs](https://github.com/Query-farm/vgi-actions) for the verify
recipe.

**2. Attach it in DuckDB** (any DuckDB with the `vgi` community extension):

```sql
INSTALL vgi FROM community;    -- one time
ATTACH 'fixed' (TYPE vgi, LOCATION '/absolute/path/to/fixedformat-worker');
SET search_path = 'fixed.main';   -- so you can call functions unqualified
```

Use an **absolute** `LOCATION` (it's resolved relative to DuckDB's working
directory).

### Compatibility & limits

- **DuckDB / vgi:** needs a DuckDB with the `vgi` community extension (the worker
  speaks Arrow IPC over the VGI protocol). Built against `vgi 0.9.5` / arrow 59;
  the prebuilt binaries are platform-specific (download the one matching your OS
  and CPU). If `ATTACH` fails with an opaque Arrow/IPC error, you most likely have
  a mismatched `vgi` extension version — update it (`UPDATE EXTENSIONS;`).
- **Not yet supported:** `COPY … TO` does not forward `CREATE SECRET` credentials
  (use `write_fixed`/`write_multi` for secret-backed cloud writes); `http(s)://`
  is **read-only** (single object — no globbing); encodings are ASCII and EBCDIC
  CP037 only (no Latin-1 / UTF-16 / other code pages yet). For multi-record-type
  files there's no `COPY FROM` form (use `INSERT INTO t SELECT record FROM
  read_multi(…)`), and `read_multi`'s `fixed` framing assumes every record type is
  padded to one common length (different fixed lengths per type — "fixed-by-type"
  — isn't supported yet; use `newline`/`rdw` framing for those).
- **Safety caps (untrusted input):** decompression is bounded by
  `max_decompressed_bytes` (16 GiB default; gzip/zstd only) to stop a
  decompression bomb, a single record by 512 MiB, and `DECIMAL` precision by 38.
  An `http(s)://` read to an internal host (cloud metadata, loopback, RFC-1918)
  is refused — set `FIXEDFORMAT_ALLOW_INTERNAL_HOSTS=1` to override.

---

## The functions

| Function | Shape | What it does |
|----------|-------|--------------|
| `unpack_fixed(rec, spec [, encoding])` | scalar → STRUCT | Parse one VARCHAR/BLOB record into a struct of fields |
| `unpack_multi(rec, spec [, encoding])` | scalar → UNION | Parse one heterogeneous record into a `UNION` (scalar `read_multi`) |
| `pack_fixed(struct, spec [, encoding])` | scalar → BLOB | Format a struct back into a fixed-width record |
| `read_fixed(path, spec [, options…])` | table function | Read a whole fixed-width file into rows |
| `read_multi(path, spec [, options…])` | table function | Read a heterogeneous (multi-record-type) file into a `UNION` column |
| `write_fixed((FROM rel), path, spec [, options…])` | table function | Write a relation out to a fixed-width file |
| `write_multi((FROM rel), path, spec [, options…])` | table function | Write a single-`UNION`-column relation back out to a heterogeneous (multi-record-type) file |
| `describe_fixed(spec [, format =>])` | table function | Introspect a spec (fields, types, offsets) without reading data |
| `describe_multi(spec)` | table function | Introspect a multi-record spec — one row per (record type, field) |

`pack_fixed` is the exact inverse of `unpack_fixed`:
`pack_fixed(unpack_fixed(rec, s), s) = rec`.

### `unpack_fixed` — parse a record

```sql
SELECT unpack_fixed('JOHN      00042', 'name:A10 qty:9(5)').qty;   -- 42

-- Over a column:
SELECT (unpack_fixed(raw_line, 'name:A10 qty:9(5)')).*
FROM my_table;
```

`rec` can be `VARCHAR` or `BLOB` (use a BLOB for binary / COMP-3 / EBCDIC data).
The third argument is the byte encoding (`'ascii'` default, or `'ebcdic'`).

### `pack_fixed` — build a record

```sql
SELECT pack_fixed({'name': 'JOHN', 'qty': 42}, 'name:A10 qty:9(5)');
-- returns BLOB 'JOHN      00042'
```

### `read_fixed` — read a file

```sql
SELECT * FROM read_fixed('data/accounts.dat', 'name:A10 qty:9(5)');

-- COBOL copybook + EBCDIC + fixed-length records:
SELECT * FROM read_fixed('data/master.dat',
    '01 REC. 05 NM PIC X(20). 05 BAL PIC S9(7)V99 COMP-3.',
    encoding => 'ebcdic', framing => 'fixed');
```

`path` may be a glob (`data/*.dat`). Options are **named**: `format`, `encoding`,
`framing`, `record_length`, `compression`.

**Compressed files are read transparently.** A `.gz` (gzip) or `.zst`
(Zstandard) file is detected from its magic bytes and decompressed before
framing — locally or from S3 — so no separate decompress step is needed:

```sql
SELECT * FROM read_fixed('data/accounts.dat.gz', 'name:A10 qty:9(5)');         -- auto
SELECT * FROM read_fixed('s3://bucket/big.dat.zst', 'name:A10 qty:9(5)');      -- auto, from S3
```

Override detection with `compression =>` `'auto'` (default), `'none'`, `'gzip'`,
or `'zstd'` — use `'none'` to force raw bytes, or name a codec for a file whose
extension doesn't match.

### `read_multi` — read a heterogeneous (multi-record-type) file

Real flat files are often heterogeneous: a **header** record, many **detail**
records, a **trailer** — each a *different* layout, picked by a "record type"
discriminator field (e.g. byte 0 = `H` / `D` / `T`). `read_multi` decodes each
record with the layout chosen by its discriminator and returns a single column
`record` of type **`UNION`** — one `STRUCT` variant per record type, the variant
names being the discriminator values.

The `spec` is a small JSON wrapper — a `discriminator` (`{offset, width}` — the
bytes that identify each record's type) plus a `records` map of tag → layout —
because the "dispatch on a discriminator" idea has no Perl/Python `unpack`
equivalent (in those languages you read the type byte and call a different
`unpack`/`struct` yourself). But **each variant's layout can be a compact
template string** (the terse `unpack` style — auto-detected, so a copybook or
inline JSON array works too), so it's far less verbose than a field array. The
`x` token is the 1-byte discriminator pad. An optional `default` tag handles
values that match no record type (otherwise an unmatched value is a hard error).

```sql
SELECT
  union_tag(record)               AS kind,           -- 'H' / 'D' / 'T'
  union_extract(record, 'D').sku  AS sku,            -- the detail STRUCT's fields
  union_extract(record, 'D').qty  AS qty
FROM read_multi('data/multi.dat', '{
  "discriminator": {"offset": 0, "width": 1},
  "records": {
    "H": "x co:A20",
    "D": "x sku:A10 qty:9(5)",
    "T": "x cnt:9(6)"
  }
}')
WHERE union_tag(record) = 'D';
```

A variant may still be the verbose JSON field array when a field needs options a
template can't express (and the two forms can mix):

```json
"D": [{"type":"filler","width":1}, {"name":"amt","type":"comp3","digits":9,"scale":2,"signed":true}]
```

`typeof(record)` is
`UNION(H STRUCT(co VARCHAR), D STRUCT(sku VARCHAR, qty BIGINT), T STRUCT(cnt BIGINT))`.
Options are **named**: `encoding`, `framing` (`newline` default / `fixed` / `rdw` /
`rdw_blocked`), `record_length` (for `fixed` framing — every record type padded to a
common length; defaults to the longest variant), and `compression`. `path` may glob
or be a cloud URL, exactly like `read_fixed`.

### `write_fixed` — write a file

`write_fixed` is a **table function**, so call it in a `FROM` clause and pass the
input relation as a subquery `(FROM …)`:

```sql
SELECT * FROM write_fixed((FROM my_table), '/tmp/out.dat', 'name:A10 qty:9(5)');
-- returns one row: (rows_written, bytes_written)

-- Compress the output (auto from the .gz/.zst extension, or compression =>):
SELECT * FROM write_fixed((FROM my_table), '/tmp/out.dat.gz', 'name:A10 qty:9(5)');
```

Named options mirror `read_fixed`: `format`, `encoding`, `framing`, plus
`compression` (`auto` / `none` / `gzip` / `zstd`) and the `s3://` overrides.

### `write_multi` — write a heterogeneous (multi-record-type) file

`write_multi` is the inverse of [`read_multi`](#read_multi--read-a-heterogeneous-multi-record-type-file):
it writes a relation whose **single column is a `UNION`** (the exact shape
`read_multi` emits) back out to a heterogeneous flat file. Each row's active
variant gives its record type (the union tag) and its `STRUCT` field values; the
matching variant layout encodes those fields and the discriminator field is
stamped with the tag. It is a **table function**, so call it in a `FROM` clause
with the input relation as a subquery `(FROM …)`, using the **same** multi-record
JSON spec as `read_multi`:

```sql
SELECT * FROM write_multi(
  (FROM read_multi('data/multi.dat', '{ "discriminator": {"offset":0,"width":1},
     "records": { "H": [...], "D": [...], "T": [...] } }')),
  '/tmp/out.dat',
  '{ "discriminator": {"offset":0,"width":1},
     "records": { "H": [...], "D": [...], "T": [...] } }');
-- returns one row: (rows_written, bytes_written)
```

The input relation must have exactly one column, a `UNION` whose variant names
match the spec's discriminator tags. Optional NAMED args mirror `write_fixed`:
`encoding =>` (`ascii`/`ebcdic`), `framing =>`
(`newline`/`fixed`/`rdw`/`rdw_blocked`), and `compression =>`. As with
`read_multi`, the discriminator must sit at a fixed offset before any
`OCCURS … DEPENDING ON` table, and `fixed` framing only aligns when every variant
is padded to a common length.

### `describe_fixed` — introspect a spec

See exactly how a spec resolves — one row per field — **without reading any
data**. Great for debugging offsets or documenting a layout:

```sql
SELECT path, sql_type, byte_offset, width, occurs
FROM describe_fixed('name:A10 qty:9(5) vals:s(3)');
-- name  VARCHAR   0  10  NULL
-- qty   BIGINT   10   5  NULL
-- vals  BIGINT[] 15   2     3
```

Columns: `path` (dotted, e.g. `item.sku`), `depth`, `kind` (codec label),
`sql_type` (the DuckDB column type), `byte_offset`, `width`, `occurs`
(OCCURS maximum, else NULL), and `depending_on` (the `OCCURS … DEPENDING ON`
controller, else NULL).

---

## `COPY … FROM` / `COPY … TO`

The worker also plugs a fixed-width format into DuckDB's `COPY` statement, so you
can load and unload tables without the `read_fixed` / `write_fixed` calls. The
format name is **catalog-qualified** by the `ATTACH` name (`fixed` below). The
spec and other settings are passed as `COPY` options; the path comes from the
statement itself.

```sql
-- Load a fixed-width file straight into a table (reader: 'fixed.fixed').
CREATE TABLE accounts (name VARCHAR, qty INTEGER);
COPY accounts FROM 'accounts.dat' (FORMAT 'fixed.fixed', spec 'name:A10 qty:9(5)');

-- Write a query/table out to a fixed-width file (writer: 'fixed.fixed_out').
COPY (SELECT name, qty FROM accounts)
  TO 'out.dat' (FORMAT 'fixed.fixed_out', spec 'name:A10 qty:9(5)');
```

Options mirror the table functions: `spec` (required), `format`, `encoding`,
`framing`, `compression` (gzip/zstd, auto-detected), plus
`endpoint`/`region`/`url_style`/`use_ssl` for `s3://` paths. On
`COPY … FROM`, decoded columns map to the target table's columns **by position**;
on `COPY … TO`, input columns map to layout fields **by name**. The reader and
writer use **different** format names (`fixed.fixed` vs `fixed.fixed_out`) because
the VGI worker SDK advertises each direction as a separate format.

> Cloud note: `COPY … FROM` resolves `CREATE SECRET` credentials per path;
> `COPY … TO` does **not** forward DuckDB secrets — use named overrides / ambient
> credentials, or `write_fixed`, for secret-backed cloud writes.

---

## Spec formats

A *spec* describes the record layout. Three formats are accepted and
auto-detected (override with `format =>` on the table functions):

### 1. Template (Perl `unpack` / Python `struct` style)

Whitespace-separated tokens, each optionally `name:`-prefixed.

```
name:A10 qty:9(5) amt:l< flags:C
```

| Code | Meaning | The count is… |
|------|---------|---------------|
| `A` / `a` / `Z` | string — space-pad / null-pad / null-terminated | the **width** in bytes |
| `9(n)` / `S9(n)` / `X(n)` | display numeric / signed / text (COBOL PIC) | the digit/char count |
| `c` `C` | int8 / uint8 | a **repeat** → LIST |
| `s` `S` | int16 / uint16 | a repeat |
| `l` `L` (or `i` `I`) | int32 / uint32 | a repeat |
| `q` `Q` | int64 / uint64 | a repeat |
| `n` `N` | uint16 / uint32, **big-endian** | a repeat |
| `v` `V` | uint16 / uint32, **little-endian** | a repeat |
| `e` `f` `d` | float16 / float32 / float64 | a repeat |
| `H` `h` | hex string, high / low nibble first | the width |
| `?` | boolean byte | a repeat |
| `x` | pad byte(s) — consumed, no output column | the width |

**Byte order:** default is big-endian. A trailing `<` / `>` on a code sets it
per-field (`l<` = little-endian int32); a standalone `<` `>` `!` `=` `@` token
sets the default for everything after it. (`!`/`>` = network/big, `<` = little,
`=`/`@` = native.)

A count is `(n)` or trailing digits: `A10` and `A(10)` are the same; `s(3)` is
three int16s returned as a `LIST`.

### 2. JSON field list

Self-documenting; good for generated specs.

```json
[
  {"name": "id",   "type": "str",   "width": 10},
  {"name": "qty",  "type": "int",   "digits": 5},
  {"name": "amt",  "type": "comp3", "digits": 9, "scale": 2, "signed": true}
]
```

Types: `str`, `int`, `decimal`, `comp3`/`packed`, `zoned`, `binary`/`comp`,
`float`/`double`/`half`, `hex`, `bool`, `pad`, and the temporal types `date` /
`time` / `datetime` (= `timestamp`). Options: `width`, `digits`, `scale`,
`signed`, `endian` (`big`/`little`), `occurs`, `justify` (`left`/`right`), `pad`,
`sign` (`leading`/`trailing`/`embedded`), and `format` (required for the temporal
types).

**Dates & times** parse a field's display bytes with a strftime `format` into a
real DuckDB `DATE` / `TIME` / `TIMESTAMP` — e.g.
`{"name": "d", "type": "date", "width": 8, "format": "%Y%m%d"}` turns `20240131`
into `2024-01-31`. (Template and copybook have no date token — COBOL dates are
plain numerics; use the JSON spec for typed dates.)

A field may instead carry a nested `fields` array, making it a **group**
(`STRUCT`; its `type` is then optional). Combined with `occurs` a group becomes a
`LIST` of `STRUCT`, so nested and repeating sub-records are expressible without a
COBOL copybook:

```json
[
  {"name": "hdr",   "type": "str", "width": 4},
  {"name": "lines", "occurs": 3, "fields": [
    {"name": "sku", "type": "str", "width": 3},
    {"name": "qty", "type": "int", "digits": 2}
  ]}
]
```

### 3. COBOL copybook

Real copybook text — paste it straight in.

```cobol
01  ACCOUNT-RECORD.
    05  ACCT-ID      PIC X(10).
    05  BALANCE      PIC S9(7)V99 COMP-3.
    05  HISTORY      OCCURS 12 PIC 9(6).
    05  RAW-DATE     PIC X(8).
    05  DATE-PARTS REDEFINES RAW-DATE.
        10  YYYY     PIC 9(4).
        10  MM       PIC 9(2).
        10  DD       PIC 9(2).
```

- Group items → `STRUCT` columns.
- `OCCURS n` → `LIST` (`UNNEST` it to get rows).
- `OCCURS [m TO] n DEPENDING ON ctrl` → a **variable-length** `LIST` whose length
  is the runtime value of the `ctrl` field (which must appear before the table).
  Records then vary in length, so the file must be `newline`- or `rdw`-framed
  (not `fixed`).
- `REDEFINES` → a `STRUCT` holding every overlapping interpretation of the same
  bytes (named after the base field).
- `USAGE COMP-3`/`PACKED-DECIMAL`, `COMP`/`COMP-4`/`COMP-5`/`BINARY`,
  `SIGN LEADING/TRAILING [SEPARATE]`, and `SYNCHRONIZED`/`SYNC` (binary items
  align to their natural boundary) are honored.
- **Edited (PICTURE-editing) numerics** — report/print-image PICs like
  `ZZ,ZZ9.99`, `$$$,$$9.99`, `9(5)CR`, `**1,234.50` decode to an exact
  `DECIMAL(p,s)` (the editing — zero-suppression, currency, commas, `CR`/`DB` and
  `+`/`-` signs — is stripped). The non-floating masks round-trip on write.

---

## Type mapping

| Field | DuckDB type |
|-------|-------------|
| text / hex | `VARCHAR` |
| display / binary integer | `BIGINT` |
| COMP-3, zoned, implied-point, **edited** decimal | `DECIMAL(p, s)` (exact) |
| float32 / float16 | `REAL` |
| float64 | `DOUBLE` |
| boolean | `BOOLEAN` |
| `date` / `time` / `datetime` (JSON spec) | `DATE` / `TIME` / `TIMESTAMP` |
| OCCURS / OCCURS DEPENDING ON | `LIST` of the above (variable-length for DEPENDING ON) |
| group / nested `fields` / REDEFINES | `STRUCT` |

## Encodings, framing & compression

- **encoding**: `ascii` (default) or `ebcdic` (code page 037).
- **framing** (how records are delimited in a file):
  - `newline` (default) — one record per line.
  - `fixed` — no delimiters; records are exactly `record_length` bytes
    (defaults to the layout length; override with `record_length =>`).
  - `rdw` — IBM variable-length: each record prefixed with a 4-byte Record
    Descriptor Word.
  - `rdw_blocked` — RDW records inside Block Descriptor Word blocks.
- **compression** — works on both sides:
  - **read** (`read_fixed` / `read_multi` / `COPY … FROM`): `auto` (default —
    detect `gzip`/`zstd` from magic bytes, else read raw), `none`, `gzip`, or
    `zstd`. Decompression happens before framing, local or `s3://` alike.
  - **write** (`write_fixed` / `write_multi` / `COPY … TO`): `auto` (default —
    `gzip` for a `.gz` destination, `zstd` for `.zst`, else raw), `none`, `gzip`,
    or `zstd`. The whole file is compressed before writing.

`read_fixed` **streams** `newline`/`fixed` files — records are framed and decoded
a batch at a time (decompressing on the fly), so memory stays flat on large
inputs instead of holding the whole file plus every decoded row; a multi-file
glob reads one file at a time. **`s3://`/`http(s)://` objects stream too**: they're
fetched in 8 MiB byte ranges on demand, so a large cloud object never materializes
whole. `rdw`/`rdw_blocked` still buffer a whole object (their length-prefix walking
needs it). Because reads stream, a malformed record deep in a file aborts the
statement after earlier batches were produced — the failed statement returns no
result, so nothing partial is committed.

**Projection pushdown:** `read_fixed` only materializes the columns your query
actually selects (mapped by name), so `SELECT one_field FROM read_fixed(…)` over a
wide layout skips building the rest.

---

## Recipes

**Split a fixed-width text column already in a table:**
```sql
SELECT (unpack_fixed(line, 'id:A8 name:A20 amt:9(9)')).*
FROM staging_lines;
```

**Convert a COBOL EBCDIC file to a Parquet file:**
```sql
COPY (
  SELECT * FROM read_fixed('mainframe.dat', '<copybook>',
                           encoding => 'ebcdic', framing => 'fixed')
) TO 'out.parquet';
```

**Flatten an OCCURS array into rows:**
```sql
SELECT r.acct_id, h.idx, h.val
FROM read_fixed('f.dat', '01 R. 05 ACCT-ID PIC X(10). 05 H OCCURS 3 PIC 9(4).') r,
     UNNEST(r.H) WITH ORDINALITY AS h(val, idx);
```

**Read variable-length records (`OCCURS … DEPENDING ON`):** the table length is
driven by an earlier count field, so the records vary in length — frame them with
`newline` (or `rdw`), not `fixed`:
```sql
SELECT N, ITEMS, TRAILER
FROM read_fixed('orders.dat',
  '01 R. 05 N PIC 9(1).
         05 ITEMS OCCURS 1 TO 9 TIMES DEPENDING ON N PIC X(2).
         05 TRAILER PIC X(3).',
  framing => 'newline');
```

**Debug a spec before running it** — see every field's type, byte offset, and
width without touching a file:
```sql
SELECT * FROM describe_fixed('01 R. 05 NM PIC X(20). 05 BAL PIC S9(7)V99 COMP-3.');
-- NM   …  VARCHAR        offset 0  width 20
-- BAL  …  DECIMAL(9,2)   offset 20 width 5
```

**Express nested / repeating records without COBOL** — give a JSON field a nested
`fields` array (a `STRUCT`), optionally with `occurs` (a `LIST` of `STRUCT`):
```sql
SELECT * FROM read_fixed('recs.dat',
  '[{"name":"hdr","type":"str","width":4},
    {"name":"lines","occurs":3,"fields":[
       {"name":"sku","type":"str","width":3},
       {"name":"qty","type":"int","digits":2}]}]');
```

**Round-trip / reformat a file** (read with one spec, write with another):
```sql
SELECT * FROM write_fixed(
  (FROM read_fixed('in.dat', 'a:A5 b:9(3)')),
  'out.dat', 'a:A8 b:9(5)');
```

---

## Gotchas

- The function is `unpack_fixed`, **not** `unpack` — `unpack` is a reserved
  keyword in DuckDB.
- `write_fixed` is a table function: use `SELECT * FROM write_fixed((FROM t), …)`,
  not `SELECT write_fixed(…)`.
- If a query **creates a table**, don't `SET search_path = 'fixed.main'` first —
  that points DDL at the read-only worker catalog. Either skip `search_path` and
  fully-qualify calls (`fixed.main.unpack_fixed(…)`), or create your tables before
  setting it.
- `pack_fixed`/`write_fixed` error if a value doesn't fit its field width — that's
  intentional (silent truncation corrupts fixed-width files).
- `read_fixed` decodes eagerly and **fails fast**: one malformed record (bad
  COMP-3 nibble, a too-short line, a ragged fixed-length stream) errors the whole
  query with a clear message rather than returning partial or corrupt rows.
- After rebuilding the worker, `DETACH fixed; ATTACH …` to pick up the new binary.

---

## Development

```sh
cargo test -p fixedformat-core   # unit tests + property-based round-trip fuzzing
cargo clippy --all-targets
./run_tests.sh                   # end-to-end SQLLogic suite (see below)
python3 data/generate_fixtures.py  # regenerate test fixtures
```

Coverage: 73 `fixedformat-core` unit tests, a `proptest` suite proving
`decode(encode(v)) == v` across every field kind (ASCII + EBCDIC), and 13 SQLLogic
files (160+ test directives) covering every function, spec format, framing mode, nested
and variable-length (`OCCURS … DEPENDING ON`) records, NULL handling, and
malformed-input behavior. Binary decoding is cross-checked against Python
`struct.pack` reference bytes.

The end-to-end suite (`test/sql/*.test`) runs against the built worker through
the haybarn DuckDB unittest runner. One-time setup:

```sh
uv tool install haybarn-unittest
uv tool install haybarn
echo "INSTALL vgi FROM community;" | uvx haybarn-cli
```

The codebase splits into a pure-logic crate and a thin adapter — see
[`CLAUDE.md`](CLAUDE.md) for architecture and conventions.
