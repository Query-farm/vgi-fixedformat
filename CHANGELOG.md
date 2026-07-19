# Changelog

All notable changes to `vgi-fixedformat` are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/).

## [0.9.0] — vgi 0.21 & metadata conformance

### Removed
- **`fixed.main.fixedformat_version()`** — the worker-version scalar is gone,
  along with the "Worker Metadata" catalog category. **Breaking:** any query
  calling it must be updated; the attached extension's own version reporting
  covers this need.

### Changed
- **VGI SDK bumped to `vgi 0.21.0` / `vgi-rpc 0.14.2`** (from 0.17.0 / 0.11.0).
  `arrow` stays on 59 — vgi 0.21 still pins `^59`, and a single arrow 59.1.0
  resolves. No worker source changes were required by the bump itself.
- `vgi.example_queries` is now a JSON list of `{description, sql}` objects
  (VGI515) rather than newline-joined SQL, and type names are backtick-quoted
  throughout the catalog metadata prose.
- Assorted dependency updates within semver: `object_store` 0.14.1, `tokio`
  1.53.0, `futures` 0.3.33, `serde` 1.0.229, `rustls` 0.23.42.

### Fixed
- Escaping of the `\x00` byte literals in the `pack_fixed` EBCDIC example query.
- `cargo audit` no longer ignores RUSTSEC-2026-0194 / RUSTSEC-2026-0195: the
  `object_store` 0.14.1 bump pulls `quick-xml` 0.41.0, which carries the fix, so
  regressions in those advisories fail CI again.

## [0.8.0] — template groups & count-prefix

### Added
- **Perl-`unpack` `(...)` groups in the template spec** — a parenthesised
  sub-template decodes to a `STRUCT`; a trailing count repeats it into a `LIST`
  of `STRUCT` (e.g. `item:(sku:A10 qty:9(5))` and `items:(…)3`). Groups nest.
- **`code/(...)` count-prefix** (Perl's `/`) — read an integer `code`, then
  *that many* group occurrences: the template spelling of `OCCURS … DEPENDING ON`.
  The count surfaces as a `<name>_count` column and the group becomes a
  count-sized `LIST` (e.g. `lines:N/(sku:A10 qty:9(5))`). These also work inside
  multi-record record specs, which accept template strings. `pack_fixed` /
  `write_fixed` round-trip both constructs back to bytes.

### Fixed
- `arrow_map` and `describe_fixed` now treat a `DEPENDING ON` field (which carries
  `occurs == None`) as a `LIST`, matching fixed-`OCCURS` fields — so a
  count-prefixed group reports `STRUCT(…)[]` rather than a bare `STRUCT`.

## [0.7.0] — multi-record extras

### Added
- **Scalar `unpack_multi(rec, spec [, encoding])`** — the scalar counterpart of
  `read_multi`: decode one heterogeneous record into a DuckDB `UNION` value
  (`union_tag` / `union_extract`).
- **`describe_multi(spec)`** — introspect a multi-record spec without reading
  data: one row per (record type, field), with the variant tag, DuckDB type, byte
  offset, width, and OCCURS info (the multi-record `describe_fixed`).

### Notes
- There is intentionally no `COPY … FROM` multi-record form — load a heterogeneous
  file with `INSERT INTO t SELECT record FROM read_multi(…)`.
- `read_multi`'s `fixed` framing still assumes every record type is padded to one
  common length; different fixed lengths per record type ("fixed-by-type" framing)
  remains a future item — use `newline`/`rdw` framing meanwhile.

## [0.6.0] — write & cloud

### Added
- **Write-side compression**: `write_fixed` / `COPY … TO` now emit gzip/zstd —
  `compression =>` `auto` (default: gzip for `.gz`, zstd for `.zst`, else raw) /
  `none` / `gzip` / `zstd`. (Replaces the previous "reject a compressed
  destination" guard.)
- **`write_multi`**: the inverse of `read_multi` — write a relation whose single
  column is a `UNION` back out to a heterogeneous multi-record-type file
  (stamps the discriminator + encodes each row with its variant layout).
- **True S3/HTTP byte-streaming**: remote objects are now read in 8 MiB byte
  ranges on demand instead of being fetched whole, so a large object streams with
  bounded memory (≈ one chunk + one batch) for newline/fixed framing.

## [0.5.0] — functionality

### Added
- **Multi-record-type files** (`read_multi`): a JSON spec with a `discriminator`
  + per-type layouts decodes each record with the layout chosen by its type and
  returns a single `record` column of DuckDB **`UNION`** (one `STRUCT` variant per
  record type — `union_tag` / `union_extract` to access). Header/detail/trailer
  files now read in one pass.
- **Date / time field type**: JSON `"date"` / `"time"` / `"datetime"` with a
  strftime `format` parse fixed-width display bytes into DuckDB `DATE` / `TIME` /
  `TIMESTAMP` (and back on write).
- **Edited (PICTURE-editing) numerics**: report/print-image PICs like
  `ZZ,ZZ9.99`, `$$$,$$9.99`, `9(5)CR`, `**1,234.50` decode to `DECIMAL(p,s)`
  (stripping the editing); the non-floating masks round-trip on write.
- **Projection pushdown** for `read_fixed`: only the selected columns are
  materialized, mapped **by name** (which also fixed a latent reorder bug in the
  positional transpose).

### Fixed
- **COBOL `SYNCHRONIZED` (SYNC) alignment**: binary items now align to their
  natural halfword/fullword/doubleword boundary via implicit slack bytes — a SYNC
  copybook previously computed wrong offsets for the item and everything after it.

## [0.4.0] — hardening

### Security / hardening (untrusted input)
- **Decompression-bomb caps.** gzip/zstd input is bounded by
  `max_decompressed_bytes` (16 GiB default, configurable on `read_fixed` /
  `COPY … FROM`), a single record by 512 MiB, and the zstd window is bounded.
  Uncompressed input is unaffected.
- **`OCCURS` count clamp.** An attacker-controlled `OCCURS … DEPENDING ON` count
  can no longer pre-allocate gigabytes; it fails fast on the first out-of-bounds
  read instead.
- **SSRF guard.** An `http(s)://` read aimed at an internal host (cloud metadata
  `169.254.169.254`, loopback, RFC-1918/ULA) is refused; override with
  `FIXEDFORMAT_ALLOW_INTERNAL_HOSTS=1`.
- **Strict write-by-name.** `pack_fixed` / `write_fixed` / `COPY … TO` now error
  on a missing or mis-named (typo'd) column instead of silently writing a blank
  field. A present-but-`NULL` value is still allowed.
- **Checked decimal arithmetic.** COMP-3 / zoned decode error on overflow instead
  of silently wrapping in release builds; `DECIMAL` precision > 38 is rejected.
- **Caps on `record_length` and spec nesting depth** (max 64) to bound allocation
  and recursion.
- **`COPY … TO` / `write_fixed` reject a `.gz`/`.zst` destination** (write-side
  compression is unsupported — no more raw bytes under a compressed name).

### Added
- Streaming `newline`/`fixed` reads (flat memory) and a record source that fetches
  each globbed remote object lazily.
- Read errors now name the source file and 1-based record number.

### Changed
- Per-batch transpose moves `Value`s instead of cloning them.
- `block_on` handles a current-thread ambient runtime without panicking.

## [0.3.0]
- Streaming reads for `newline` / `fixed` framing (peak memory ≈ one batch).

## [0.2.0]
- Transparent gzip/zstd decompression on the read path (`read_fixed` /
  `COPY … FROM`), auto-detected by magic bytes or forced via `compression =>`.

## [0.1.1]
- Internal: removed a brittle hardcoded version assertion from the test suite.

## [0.1.0]
- First release: `unpack_fixed` / `pack_fixed` scalars; `read_fixed` /
  `write_fixed` / `describe_fixed` table functions; `COPY … FROM` / `COPY … TO`;
  template / JSON / COBOL-copybook specs; ASCII + EBCDIC; COMP-3 / zoned decimals;
  `OCCURS` / `OCCURS … DEPENDING ON` / `REDEFINES`; local + `s3://` + `http(s)://`
  paths. Signed, multi-platform release binaries.
