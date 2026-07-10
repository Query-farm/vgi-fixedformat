//! The `fixedformat` VGI worker.
//!
//! A standalone binary DuckDB launches and talks to over Apache Arrow IPC. It
//! brings Perl-`unpack` / Python-`struct` / COBOL-copybook fixed-width parsing
//! and formatting to SQL under the catalog `fixed`, schema `main`:
//!
//! - `fixed.main.unpack_fixed(rec, spec)` — parse a string/blob into a STRUCT
//! - `fixed.main.pack_fixed(struct, spec)` — format a STRUCT back into a BLOB
//! - `fixed.main.read_fixed(path, spec, ...)` — scan a fixed-width file
//! - `fixed.main.write_fixed((FROM rel), path, spec, ...)` — write one out

mod arrow_map;
mod buffering;
mod cloud;
mod copy_from;
mod copy_to;
mod meta;
mod options;
mod reader;
mod record_writer;
mod scalar;
mod table;
mod value_in;

use vgi::catalog::{CatSchema, CatView, CatalogModel};
use vgi::Worker;

/// Catalog + schema metadata (description, provenance) surfaced to DuckDB and
/// the `vgi-lint` metadata-quality linter. The function objects themselves are
/// served from the registered scalars / table / buffering functions; this only
/// adds catalog/schema-level comments and tags.
fn catalog_metadata(name: &str) -> CatalogModel {
    CatalogModel {
        name: name.to_string(),
        comment: Some(
            "Fixed-width / Perl-unpack / Python-struct / COBOL-copybook record parsing and \
             formatting for SQL."
                .to_string(),
        ),
        tags: vec![
            (
                "vgi.title".to_string(),
                "Fixed-Width & COBOL Copybook Codec".to_string(),
            ),
            (
                "vgi.keywords".to_string(),
                crate::meta::keywords_json(
                    "fixed-width, fixed format, unpack, pack, struct, perl unpack, python struct, \
                     COBOL, copybook, mainframe, EBCDIC, COMP-3, packed decimal, zoned decimal, \
                     RDW, flat file, record layout, parse, encode, describe, introspect, \
                     nested struct, OCCURS, OCCURS DEPENDING ON, variable-length, REDEFINES",
                ),
            ),
            (
                "vgi.doc_llm".to_string(),
                "Parse and format fixed-width / flat-file records directly in SQL, with no \
                 external ETL step. A layout spec describes how a record's bytes map to typed \
                 columns; from it the worker can decode a record string or blob into a typed \
                 STRUCT, re-encode a STRUCT back to the original record bytes, scan or write whole \
                 fixed-width files (local, `s3://`, or `http(s)://`), and introspect how a spec \
                 resolves before touching data. Layouts are given three interchangeable, \
                 auto-detected ways: Perl/Python `unpack` template strings, JSON field specs \
                 (which may nest a `fields` array for STRUCT / LIST-of-STRUCT sub-records), or \
                 COBOL copybooks. They support ASCII or EBCDIC (CP037) encoding; packed (COMP-3), \
                 zoned, and implied-point decimals; nested groups, REDEFINES, and OCCURS / \
                 `OCCURS … DEPENDING ON` variable-length tables; heterogeneous multi-record-type \
                 files selected by a discriminator (a UNION per record type); and four \
                 record-framing modes — newline, fixed, rdw, and rdw_blocked. Decode and encode \
                 are exact inverses, so decoding a record then re-encoding it reproduces the \
                 original bytes. Zero-config defaults are newline framing and ASCII encoding, so \
                 the common case is just a record and a spec; the spec format is auto-detected but \
                 can be forced when a layout is ambiguous. Reach for it to ingest or emit \
                 mainframe and legacy flat-file data — COBOL copybook extracts, EBCDIC/COMP-3 host \
                 feeds, RDW-framed files — straight from SQL."
                    .to_string(),
            ),
            (
                "vgi.doc_md".to_string(),
                "# fixed\n\nFixed-width / flat-file record parsing and formatting over Apache \
                 Arrow. Brings Perl-`unpack`, Python-`struct`, and COBOL-copybook style layouts to \
                 SQL so you can ingest and emit mainframe and legacy flat-file data without an \
                 external ETL step.\n\nA layout spec is given in one of three auto-detected \
                 formats — a Perl/Python `unpack` **template** string (e.g. `A10 N s>`), a **JSON** \
                 field list, or a COBOL **copybook** — and maps each field to a typed column \
                 (BIGINT / REAL / DOUBLE / VARCHAR / BOOLEAN, `DECIMAL(p,s)` for COMP-3 / zoned / \
                 implied-point numbers, LIST for `OCCURS` / `OCCURS … DEPENDING ON`, STRUCT for \
                 groups, nested JSON `fields`, and REDEFINES). \
                 Encodings are `ascii` (default) or `ebcdic` (CP037); record framing is `newline` \
                 (default), `fixed`, `rdw`, or `rdw_blocked`. The spec format is auto-detected from \
                 the spec text; on the table functions you can force it with `format =>` \
                 ('template' / 'json' / 'copybook') when a layout would otherwise be ambiguous. \
                 With the defaults (newline framing, ascii encoding) the common call is just \
                 `(record, spec)`.\n\nDecoding a record into a STRUCT and re-encoding that STRUCT \
                 back to bytes are exact inverses, and whole files can be scanned, written, or \
                 introspected — including heterogeneous multi-record-type files (a UNION per \
                 record type) — without leaving SQL."
                    .to_string(),
            ),
            // Fixed agent-suitability suite run by `vgi-lint simulate` (2 single-call
            // smoke tests + 2 multi-concept tasks: a multi-file glob aggregate and the
            // headline EBCDIC/COMP-3 mainframe decode). Each prompt is shown to the
            // simulated analyst; the hidden reference_sql is the canonical solution,
            // re-run live to grade by deterministic result comparison. Prompts name
            // their output columns (grading is strict on names/values/order). The
            // file-based tasks use repo-relative `data/...` paths, so run `vgi-lint
            // simulate` from the repo root where the test fixtures live.
            (
                "vgi.agent_test_tasks".to_string(),
                crate::meta::agent_test_tasks_json(&[
                    (
                        "unpack_single",
                        "I have one fixed-width record 'JOHN      00042' where the first 10 \
                         characters are an account name and the next 5 are a zero-padded quantity. \
                         Parse it and return the quantity as a single integer column named qty.",
                        "SELECT (fixed.main.unpack_fixed('JOHN      00042', 'name:A10 qty:9(5)'))\
                         .qty AS qty",
                    ),
                    (
                        "profile_large",
                        "The file data/large.dat is a newline-delimited fixed-width feed where each \
                         record is a single 7-digit zero-padded id. Profile it: return one row with \
                         a column named records (the record count), a column named min_id, and a \
                         column named max_id.",
                        "SELECT count(*) AS records, min(id) AS min_id, max(id) AS max_id \
                         FROM fixed.main.read_fixed('data/large.dat', 'id:9(7)')",
                    ),
                    (
                        "glob_total",
                        "A nightly job drops one or more fixed-width account files into data/, \
                         named acct1.dat, acct2.dat, and so on. Each record is a 10-character \
                         account name followed by a 5-digit zero-padded quantity. Read all of the \
                         acct*.dat files at once and return the total quantity across every record \
                         as a single column named total_qty.",
                        "SELECT sum(qty) AS total_qty \
                         FROM fixed.main.read_fixed('data/acct*.dat', 'name:A10 qty:9(5)')",
                    ),
                    (
                        "ebcdic_comp3_max",
                        "We received a mainframe extract at data/ebcdic_comp3.dat. It is \
                         EBCDIC-encoded and fixed-length (no line delimiters between records). \
                         Each record is a 5-byte name (PIC X(5)) followed by a signed \
                         packed-decimal amount PIC S9(3)V99 COMP-3. Decode the file and return the \
                         largest amount as a single column named max_amount.",
                        "SELECT max(AMT) AS max_amount FROM fixed.main.read_fixed(\
                         'data/ebcdic_comp3.dat', \
                         '01 R. 05 NM PIC X(5). 05 AMT PIC S9(3)V99 COMP-3.', \
                         encoding => 'ebcdic', framing => 'fixed')",
                    ),
                    (
                        "pack_ascii_record",
                        "We need to emit a single fixed-width record for an export. The layout is a \
                         10-character left-justified, space-padded account name followed by a \
                         5-digit zero-padded quantity. Build the record for account name 'ALICE' \
                         with quantity 5 and return the resulting record text as a single column \
                         named record.",
                        "SELECT fixed.main.pack_fixed({'name': 'ALICE', 'qty': 5}, \
                         'name:A10 qty:9(5)')::VARCHAR AS record",
                    ),
                    (
                        "pack_ebcdic_comp3_hex",
                        "A mainframe ingest job needs an account record encoded the way the host \
                         expects it: the name as a 5-byte EBCDIC field (PIC X(5)) followed by a \
                         signed packed-decimal amount PIC S9(3)V99 COMP-3. Encode name 'ACME' with \
                         amount 123.45 and return the resulting record bytes as an uppercase hex \
                         string in a single column named record_hex.",
                        "SELECT hex(fixed.main.pack_fixed({'NM': 'ACME', 'AMT': 123.45}, \
                         '01 R. 05 NM PIC X(5). 05 AMT PIC S9(3)V99 COMP-3.', 'ebcdic')) \
                         AS record_hex",
                    ),
                    (
                        "write_accounts_file",
                        "Export two accounts to a newline-delimited fixed-width file at \
                         data/_agent_write.dat, where each record is a 10-character left-justified \
                         name followed by a 5-digit zero-padded quantity: ALICE with quantity 5 and \
                         BOB with quantity 999. Return the write summary with a column named \
                         rows_written and a column named bytes_written.",
                        "SELECT rows_written, bytes_written FROM fixed.main.write_fixed(\
                         (FROM (VALUES ('ALICE', 5), ('BOB', 999)) AS v(name, qty)), \
                         'data/_agent_write.dat', 'name:A10 qty:9(5)')",
                    ),
                    (
                        "worker_version",
                        "Before relying on the fixed-format worker in a pipeline, an analyst wants \
                         to record which build is attached. Return the worker's version string as \
                         a single row with one column named version.",
                        "SELECT fixed.main.fixedformat_version() AS version",
                    ),
                    (
                        "describe_fixed_offset",
                        "I have the fixed-width layout 'name:A10 qty:9(5)' — a 10-character name \
                         followed by a 5-digit zero-padded quantity. Without reading any data, \
                         work out the byte offset at which the qty field begins and return it as a \
                         single column named qty_offset.",
                        "SELECT byte_offset AS qty_offset FROM \
                         fixed.main.describe_fixed('name:A10 qty:9(5)') WHERE path = 'qty'",
                    ),
                    (
                        "describe_multi_field_count",
                        "This multi-record layout has a 1-byte record-type discriminator at offset \
                         0 and two record types: a header 'H' with one field, and a detail 'D' \
                         with a 10-char sku and a 5-digit qty. The spec is \
                         {\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":\
                         [{\"name\":\"co\",\"type\":\"str\",\"width\":20}],\"D\":[{\"name\":\
                         \"sku\",\"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\
                         \"digits\":5}]}}. Without reading any data, return how many fields the 'D' \
                         record type declares, as a single column named field_count.",
                        "SELECT count(*) AS field_count FROM fixed.main.describe_multi(\
                         '{\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":\
                         [{\"name\":\"co\",\"type\":\"str\",\"width\":20}],\"D\":[{\"name\":\
                         \"sku\",\"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\
                         \"digits\":5}]}}') WHERE record_type = 'D'",
                    ),
                    (
                        "read_multi_detail_count",
                        "The file data/multi.dat is a heterogeneous fixed-width feed whose records \
                         have a 1-byte record-type tag at offset 0: 'H' header, 'D' detail, 'T' \
                         trailer. Each record's first byte is the tag; a detail record is the tag \
                         plus a 10-char sku and a 5-digit qty. The layout spec is \
                         {\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":\
                         [{\"type\":\"filler\",\"width\":1},{\"name\":\"co\",\"type\":\"str\",\
                         \"width\":20}],\"D\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"sku\",\
                         \"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\
                         \"digits\":5}],\"T\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"cnt\",\
                         \"type\":\"int\",\"digits\":6}]}}. Read the file and return how many \
                         detail ('D') records it contains, as a single column named detail_count.",
                        "SELECT count(*) AS detail_count FROM fixed.main.read_multi(\
                         'data/multi.dat', '{\"discriminator\":{\"offset\":0,\"width\":1},\
                         \"records\":{\"H\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"co\",\
                         \"type\":\"str\",\"width\":20}],\"D\":[{\"type\":\"filler\",\"width\":1},\
                         {\"name\":\"sku\",\"type\":\"str\",\"width\":10},{\"name\":\"qty\",\
                         \"type\":\"int\",\"digits\":5}],\"T\":[{\"type\":\"filler\",\"width\":1},\
                         {\"name\":\"cnt\",\"type\":\"int\",\"digits\":6}]}}') WHERE \
                         union_tag(record) = 'D'",
                    ),
                    (
                        "unpack_multi_qty",
                        "I have one heterogeneous record 'DWIDGET    00042'. Its first byte is a \
                         record-type tag; 'D' means a detail record laid out as the tag byte, a \
                         10-character sku, then a 5-digit zero-padded qty. The multi-record spec is \
                         {\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":\
                         [{\"type\":\"filler\",\"width\":1},{\"name\":\"co\",\"type\":\"str\",\
                         \"width\":20}],\"D\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"sku\",\
                         \"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\
                         \"digits\":5}]}}. Decode the record and return its qty as a single column \
                         named qty.",
                        "SELECT union_extract(fixed.main.unpack_multi('DWIDGET    00042', \
                         '{\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":\
                         [{\"type\":\"filler\",\"width\":1},{\"name\":\"co\",\"type\":\"str\",\
                         \"width\":20}],\"D\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"sku\",\
                         \"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\
                         \"digits\":5}]}}'), 'D').qty AS qty",
                    ),
                    (
                        "write_multi_roundtrip",
                        "Round-trip the heterogeneous file data/multi.dat: read it into its UNION \
                         representation and write it straight back out to \
                         data/_task_multi_out.dat using the same multi-record layout, then report \
                         how many records were written as a single column named rows_written. The \
                         layout spec (for both the read and the write) is \
                         {\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":\
                         [{\"type\":\"filler\",\"width\":1},{\"name\":\"co\",\"type\":\"str\",\
                         \"width\":20}],\"D\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"sku\",\
                         \"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\
                         \"digits\":5}],\"T\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"cnt\",\
                         \"type\":\"int\",\"digits\":6}]}}.",
                        "SELECT rows_written FROM fixed.main.write_multi((FROM \
                         fixed.main.read_multi('data/multi.dat', '{\"discriminator\":{\"offset\":0,\
                         \"width\":1},\"records\":{\"H\":[{\"type\":\"filler\",\"width\":1},\
                         {\"name\":\"co\",\"type\":\"str\",\"width\":20}],\"D\":[{\"type\":\
                         \"filler\",\"width\":1},{\"name\":\"sku\",\"type\":\"str\",\"width\":10},\
                         {\"name\":\"qty\",\"type\":\"int\",\"digits\":5}],\"T\":[{\"type\":\
                         \"filler\",\"width\":1},{\"name\":\"cnt\",\"type\":\"int\",\"digits\":6}]}}\
                         ')), 'data/_task_multi_out.dat', '{\"discriminator\":{\"offset\":0,\
                         \"width\":1},\"records\":{\"H\":[{\"type\":\"filler\",\"width\":1},\
                         {\"name\":\"co\",\"type\":\"str\",\"width\":20}],\"D\":[{\"type\":\
                         \"filler\",\"width\":1},{\"name\":\"sku\",\"type\":\"str\",\"width\":10},\
                         {\"name\":\"qty\",\"type\":\"int\",\"digits\":5}],\"T\":[{\"type\":\
                         \"filler\",\"width\":1},{\"name\":\"cnt\",\"type\":\"int\",\"digits\":6}]}}\
                         ')",
                    ),
                    (
                        "spec_reference_framing",
                        "Using the fixed-format worker's built-in reference table, list how many \
                         record-framing modes it supports. Return the count as a single column \
                         named framing_modes.",
                        "SELECT count(*) AS framing_modes FROM fixed.main.spec_reference WHERE \
                         topic = 'framing'",
                    ),
                ]),
            ),
            ("vgi.author".to_string(), "Query.Farm".to_string()),
            (
                "vgi.copyright".to_string(),
                "Copyright 2026 Query Farm LLC - https://query.farm".to_string(),
            ),
            ("vgi.license".to_string(), "MIT".to_string()),
            (
                "vgi.support_contact".to_string(),
                "https://github.com/Query-farm/vgi-fixedformat/issues".to_string(),
            ),
            (
                "vgi.support_policy_url".to_string(),
                "https://github.com/Query-farm/vgi-fixedformat/blob/main/README.md".to_string(),
            ),
        ],
        source_url: Some("https://github.com/Query-farm/vgi-fixedformat".to_string()),
        schemas: vec![CatSchema {
            name: "main".to_string(),
            comment: Some(
                "Fixed-width / copybook parsing, formatting, reading, and writing functions."
                    .to_string(),
            ),
            tags: vec![
                ("vgi.title".to_string(), "Fixed Format — main".to_string()),
                (
                    "vgi.keywords".to_string(),
                    crate::meta::keywords_json(
                        "fixed-width, unpack_fixed, pack_fixed, read_fixed, write_fixed, \
                         describe_fixed, copybook, template, struct, EBCDIC, COMP-3, mainframe, \
                         flat file, nested struct, OCCURS DEPENDING ON, variable-length",
                    ),
                ),
                // VGI123 classifying tags (bare keys: domain/category/topic) for faceting.
                ("domain".to_string(), "data-engineering".to_string()),
                ("category".to_string(), "parsing-and-serialization".to_string()),
                ("topic".to_string(), "fixed-width-records".to_string()),
                // VGI408/VGI413 navigation registry: the ordered category sections
                // this schema groups its objects into. Each object carries a
                // matching `vgi.category` (VGI409/VGI411); every category has at
                // least one member (VGI412).
                (
                    "vgi.categories".to_string(),
                    "[{\"name\":\"Encode & Decode\",\"description\":\"Decode a single record \
                     into a typed STRUCT and encode a STRUCT back into record bytes (the exact \
                     inverse), including heterogeneous multi-record UNION values.\"},\
                     {\"name\":\"File Read & Write\",\"description\":\"Scan whole fixed-width \
                     files into rows and write relations back out to fixed-width files, local or \
                     cloud (s3://, http(s)://), including multi-record-type files.\"},\
                     {\"name\":\"COPY Integration\",\"description\":\"Load and unload \
                     fixed-width files through DuckDB's COPY … FROM / COPY … TO statements.\"},\
                     {\"name\":\"Layout Introspection\",\"description\":\"Inspect how a layout \
                     spec resolves — fields, types, byte offsets, OCCURS — without reading any \
                     data.\"},\
                     {\"name\":\"Worker Metadata\",\"description\":\"Report information about \
                     the worker itself, such as its version.\"}]"
                        .to_string(),
                ),
                (
                    "vgi.doc_llm".to_string(),
                    "Functions for working with fixed-width / flat-file records in SQL: decode a \
                     record into a typed STRUCT, encode a STRUCT back into record bytes (the exact \
                     inverse), scan a fixed-width file into rows, write a relation to a \
                     fixed-width file, decode or emit heterogeneous multi-record-type files (a \
                     UNION per record type, selected by a discriminator), and introspect how a \
                     spec resolves (fields, types, byte offsets) without reading data. Layouts are \
                     template strings, JSON specs (which may nest a `fields` array for \
                     STRUCT/LIST-of-STRUCT sub-records), or COBOL copybooks (auto-detected; force \
                     with `format =>` on the table functions). Field kinds map to columns as \
                     text/hex → VARCHAR, integers → BIGINT, COMP-3/zoned/implied-point → \
                     DECIMAL(p,s), OCCURS and OCCURS DEPENDING ON → LIST, \
                     group/nested-fields/REDEFINES → STRUCT. Encodings are ascii (default) or \
                     ebcdic (CP037); framing is newline (default), fixed, rdw, or rdw_blocked. \
                     With the defaults the common call is just `(record, spec)`."
                        .to_string(),
                ),
                (
                    "vgi.doc_md".to_string(),
                    "# fixed.main\n\nThe single (and only) schema for the `fixed` worker — the \
                     catalog name matches the `ATTACH` name, so qualify calls as \
                     `fixed.main.<fn>(...)`.\n\nIt provides functions for parsing a record into a \
                     STRUCT and encoding a STRUCT back to bytes (an exact inverse pair), scanning \
                     and emitting whole fixed-width files (including heterogeneous \
                     multi-record-type files as a UNION per record type), and introspecting a \
                     layout — one row per field with its dotted path, type, byte offset, width, \
                     and OCCURS info — without reading data.\n\nLayouts are given as Perl/Python \
                     `unpack` templates, JSON field specs (which may nest a `fields` array for \
                     STRUCT/LIST-of-STRUCT sub-records), or COBOL copybooks (auto-detected; \
                     override with `format =>` on the table functions). Encodings are `ascii` \
                     (default) or `ebcdic` (CP037); record framing is `newline` (default), \
                     `fixed`, `rdw`, or `rdw_blocked`.\n\nField kinds map to columns as:\n\n\
                     - text / hex → VARCHAR\n\
                     - integers → BIGINT\n\
                     - COMP-3 / zoned / implied-point → DECIMAL(p,s)\n\
                     - OCCURS and OCCURS … DEPENDING ON → LIST\n\
                     - group / nested-fields / REDEFINES → STRUCT"
                        .to_string(),
                ),
                // VGI506 representative example queries for the schema.
                (
                    "vgi.example_queries".to_string(),
                    "SELECT fixed.main.unpack_fixed('JohnDoe  00042', 'A8 N');\n\
                     SELECT fixed.main.pack_fixed({'name': 'Jo', 'id': 7}, 'A2 N');\n\
                     SELECT fixed.main.fixedformat_version();\n\
                     SELECT * FROM fixed.main.describe_fixed('name:A10 qty:9(5)');\n\
                     SELECT * FROM fixed.main.read_fixed('data/*.dat', 'A10 N');\n\
                     SELECT * FROM fixed.main.write_fixed((FROM tbl), '/tmp/out.dat', 'A10 N');"
                        .to_string(),
                ),
            ],
            // A browsable, credential-free reference view (VGI146): it lets an
            // agent SELECT the worker's own vocabulary — the three spec formats,
            // the encoding / framing / compression option tokens, and the field
            // kind → DuckDB type mapping — before it has to construct a `spec`
            // argument for the table functions. Backed by a literal VALUES list so
            // it scans instantly and needs no data file or secret.
            views: vec![CatView {
                name: "spec_reference".to_string(),
                definition: "SELECT * FROM (VALUES \
                    ('spec_format', 'template', '', 'Perl/Python unpack template string, e.g. \
                     name:A10 qty:9(5).'), \
                    ('spec_format', 'json', '', 'JSON field list (a field may nest a `fields` \
                     array for STRUCT / LIST-of-STRUCT sub-records).'), \
                    ('spec_format', 'copybook', '', 'COBOL copybook text (PIC clauses, OCCURS, \
                     REDEFINES, COMP-3).'), \
                    ('encoding', 'ascii', '', 'Plain ASCII / Latin-1 bytes (the default).'), \
                    ('encoding', 'ebcdic', '', 'EBCDIC code page CP037 (IBM mainframe); also \
                     governs zoned / COMP-3 sign nibbles.'), \
                    ('framing', 'newline', '', 'Records separated by a newline byte (the \
                     default).'), \
                    ('framing', 'fixed', '', 'Back-to-back records of equal byte length (no \
                     separator).'), \
                    ('framing', 'rdw', '', 'IBM variable-length records, each prefixed by a \
                     Record Descriptor Word.'), \
                    ('framing', 'rdw_blocked', '', 'RDW records grouped into blocks, each \
                     prefixed by a Block Descriptor Word.'), \
                    ('compression', 'auto', '', 'Detect gzip / zstd from magic bytes on read; \
                     derive from the file extension on write (the default).'), \
                    ('compression', 'none', '', 'No compression (raw bytes).'), \
                    ('compression', 'gzip', '', 'gzip (DEFLATE) whole-file compression.'), \
                    ('compression', 'zstd', '', 'Zstandard whole-file compression.'), \
                    ('field_kind', 'text / hex', 'VARCHAR', 'A character or hex field decoded to \
                     a string.'), \
                    ('field_kind', 'integer', 'BIGINT', 'A zoned or binary integer field.'), \
                    ('field_kind', 'float / double', 'DOUBLE', 'An IEEE floating-point field.'), \
                    ('field_kind', 'packed / zoned / implied-point', 'DECIMAL(p,s)', 'A COMP-3 \
                     packed, zoned, or implied-decimal-point number.'), \
                    ('field_kind', 'boolean', 'BOOLEAN', 'A single-flag boolean field.'), \
                    ('field_kind', 'occurs / repeat', 'LIST', 'A repeating field (OCCURS or \
                     OCCURS DEPENDING ON) as a LIST of the element type.'), \
                    ('field_kind', 'group / redefines', 'STRUCT', 'A group item, nested fields, \
                     or REDEFINES rendered as a STRUCT.') \
                    ) AS t(topic, token, maps_to, description)"
                    .to_string(),
                comment: Some(
                    "Reference registry of the worker's spec formats, encoding / framing / \
                     compression option tokens, and field-kind to DuckDB-type mappings."
                        .to_string(),
                ),
                tags: vec![
                    (
                        "vgi.title".to_string(),
                        "Fixed Format — Spec Reference".to_string(),
                    ),
                    crate::meta::category("Layout Introspection"),
                    ("domain".to_string(), "data-engineering".to_string()),
                    (
                        "vgi.keywords".to_string(),
                        crate::meta::keywords_json(
                            "reference, vocabulary, spec format, template, json, copybook, \
                             encoding, ascii, ebcdic, framing, newline, rdw, compression, gzip, \
                             zstd, field kind, DuckDB type, mapping, cheat sheet",
                        ),
                    ),
                    (
                        "vgi.doc_llm".to_string(),
                        "A static lookup table of the fixed-format worker's own vocabulary, so an \
                         agent can discover the valid option tokens and type mappings before \
                         building a `spec`. One row per (topic, token): the `topic` column groups \
                         the rows into `spec_format` (the three auto-detected layout formats — \
                         template, json, copybook), `encoding` (ascii / ebcdic), `framing` \
                         (newline / fixed / rdw / rdw_blocked), `compression` (auto / none / gzip \
                         / zstd), and `field_kind` (how each layout field kind maps to a DuckDB \
                         column type). The `token` column is the accepted value (or field-kind \
                         name), `maps_to` is the resulting DuckDB type for field_kind rows (empty \
                         otherwise), and `description` explains it. Query it to enumerate the \
                         legal `encoding =>` / `framing =>` / `compression =>` / `format =>` \
                         values, or to see which DuckDB type a COMP-3 or OCCURS field becomes."
                            .to_string(),
                    ),
                    (
                        "vgi.doc_md".to_string(),
                        "# spec_reference\n\nA browsable, credential-free registry of the \
                         `fixed` worker's own vocabulary — query it to discover valid option \
                         values and type mappings before constructing a layout `spec`.\n\nEach row \
                         is a (topic, token) pair:\n\n- **topic** — one of `spec_format`, \
                         `encoding`, `framing`, `compression`, or `field_kind`.\n- **token** — the \
                         accepted value for that topic (or the field-kind name).\n- **maps_to** — \
                         for `field_kind` rows, the DuckDB column type the kind maps to; empty \
                         otherwise.\n- **description** — a one-line explanation.\n\nFor example, \
                         filter `topic = 'framing'` to list the record-framing modes, or \
                         `topic = 'field_kind'` to see how a COMP-3 or OCCURS field is typed."
                            .to_string(),
                    ),
                    (
                        "vgi.example_queries".to_string(),
                        "[{\"description\":\"List the record-framing modes the worker accepts, \
                         with their meanings.\",\"sql\":\"SELECT token, description FROM \
                         fixed.main.spec_reference WHERE topic = 'framing' ORDER BY token\"},\
                         {\"description\":\"Show how each layout field kind maps to a DuckDB \
                         column type.\",\"sql\":\"SELECT token AS field_kind, maps_to AS \
                         duckdb_type FROM fixed.main.spec_reference WHERE topic = 'field_kind' \
                         ORDER BY token\"}]"
                            .to_string(),
                    ),
                ],
                column_comments: vec![
                    (
                        "topic".to_string(),
                        "The vocabulary group: 'spec_format', 'encoding', 'framing', \
                         'compression', or 'field_kind'."
                            .to_string(),
                    ),
                    (
                        "token".to_string(),
                        "The accepted option value within the topic, or the field-kind name for \
                         'field_kind' rows."
                            .to_string(),
                    ),
                    (
                        "maps_to".to_string(),
                        "For 'field_kind' rows, the DuckDB column type the kind maps to; an empty \
                         string for the option-token rows."
                            .to_string(),
                    ),
                    (
                        "description".to_string(),
                        "A one-line explanation of the token.".to_string(),
                    ),
                ],
            }],
            macros: Vec::new(),
            tables: Vec::new(),
        }],
        ..Default::default()
    }
}

fn main() {
    // Logs MUST go to stderr — stdout is the Arrow-IPC channel.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().filter_or("VGI_LOG", "info"))
        .format_timestamp_millis()
        .try_init();

    // The catalog name DuckDB sees in `ATTACH 'fixed' (TYPE vgi, …)`. Default to
    // `fixed`, but honor an explicit override so a test harness can rename it.
    if std::env::var_os("VGI_WORKER_CATALOG_NAME").is_none() {
        std::env::set_var("VGI_WORKER_CATALOG_NAME", "fixed");
    }
    let catalog_name =
        std::env::var("VGI_WORKER_CATALOG_NAME").unwrap_or_else(|_| "fixed".to_string());

    let mut worker = Worker::new();
    scalar::register(&mut worker);
    table::register(&mut worker);
    buffering::register(&mut worker);
    copy_from::register(&mut worker);
    copy_to::register(&mut worker);
    worker.set_catalog(catalog_metadata(&catalog_name));
    worker.run();
}
