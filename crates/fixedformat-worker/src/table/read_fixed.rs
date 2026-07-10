//! `read_fixed(path, spec [, format =>, encoding =>, framing =>, record_length =>])`
//! — scan a fixed-width file into typed rows.
//!
//! `path` may be a glob. Records are framed per the `framing` option (newline /
//! fixed / RDW), then each field is decoded into a typed column (LIST/STRUCT for
//! OCCURS / group / folded REDEFINES).

use std::sync::Arc;

use arrow_schema::{Schema, SchemaRef};
use fixedformat_core::Layout;
use vgi::arguments::Arguments;
use vgi::secrets::SecretLookup;
use vgi::table_function::{TableFunction, TableProducer};
use vgi::{ArgSpec, BindParams, BindResponse, FunctionMetadata, ProcessParams};
use vgi_rpc::Result;

use crate::arrow_map::layout_fields;
use crate::cloud;
use crate::options;

pub struct ReadFixed;

fn output_schema(layout: &Layout) -> Result<SchemaRef> {
    let fields = layout_fields(layout)?;
    Ok(Arc::new(Schema::new(fields)))
}

/// Record length to use for fixed framing (override or layout-derived).
fn record_length(args: &Arguments, layout: &Layout) -> usize {
    args.named_i64("record_length")
        .map(|n| n as usize)
        .unwrap_or(layout.record_len)
}

impl TableFunction for ReadFixed {
    fn name(&self) -> &str {
        "read_fixed"
    }

    fn metadata(&self) -> FunctionMetadata {
        let mut tags = crate::meta::object_tags(
            "Read Fixed-Width File",
            "Scan a fixed-width / flat-file data file into typed rows. `path` may be a glob (e.g. \
             '/data/*.dat'); matching files are read in sorted order and their rows concatenated. \
             Each record is framed per the named `framing =>` argument ('newline' the default, \
             'fixed', 'rdw', or 'rdw_blocked') and decoded into a row of typed columns according \
             to the layout `spec` — a Perl/Python `unpack` template, a JSON field list, or a COBOL \
             copybook (format auto-detected unless you pass `format =>` 'template' / 'json' / \
             'copybook'). In a template spec, prefix each field with a name to set its column \
             name, e.g. `'name:A10 qty:9(5)'`; unnamed fields become positional `field_1`, \
             `field_2`, …. The named `encoding =>` argument is 'ascii' (default) or 'ebcdic' \
             (CP037). The named `record_length =>` argument is the per-record length in BYTES; it \
             is only used for 'fixed' framing (back-to-back records of equal length) and defaults \
             to the length implied by the `spec`; it is ignored for the other framings. `format`, \
             `encoding`, `framing`, and `record_length` are all NAMED arguments. The returned \
             column set is dynamic: it is determined by the spec, with OCCURS becoming LIST \
             columns and groups / REDEFINES becoming STRUCT columns. Use it to ingest mainframe or \
             legacy flat-file data into SQL. This is the file-scanning counterpart of unpack_fixed \
             and the inverse of write_fixed.",
            "Scan a fixed-width file into rows, decoding each record per the layout `spec` \
             (template, JSON, or COBOL copybook). `path` may glob (e.g. \
             `read_fixed('/data/*.dat', 'A10 N')`), reading matching files in sorted order. \
             Optional NAMED args: `format =>`, `encoding =>` ('ascii'/'ebcdic'), `framing =>` \
             ('newline'/'fixed'/'rdw'/'rdw_blocked'), and `record_length =>` (per-record length in \
             bytes, used only for `fixed` framing). The returned columns are dynamic — they depend \
             on the spec, with OCCURS → LIST and group/REDEFINES → STRUCT.",
            "read fixed, scan, fixed-width file, flat file, ingest, copybook, mainframe, EBCDIC, \
             RDW, rdw_blocked, record_length, COMP-3, glob, file to rows, table function",
        );
        tags.push(crate::meta::category("File Read & Write"));
        // VGI307/VGI326: the result columns are dynamic — one per top-level field
        // of the layout `spec` — so the schema is declared as
        // `vgi.result_dynamic_columns_md`, with a representative Name|Type|
        // Description variant table plus the field-kind → type mapping. Runnable
        // queries live in `vgi.example_queries` below (kept out of here so the
        // coverage/execution rules grade them).
        tags.push((
            "vgi.result_dynamic_columns_md".into(),
            "The returned columns are **dynamic** — determined by the layout `spec`, one column \
             per top-level field. Column names come from the field names in the spec — name your \
             fields (e.g. `name:A10 qty:9(5)`); **unnamed** template fields become positional \
             `field_1`, `field_2`, …. Column types follow the field kind: text/hex → `VARCHAR`, \
             integer → `BIGINT`, float/double → `REAL`/`DOUBLE`, COMP-3 / zoned / implied-point \
             decimal → `DECIMAL(p,s)`, `?` boolean → `BOOLEAN`, OCCURS / repeat → `LIST` of the \
             element type, and group / REDEFINES → `STRUCT` of the child fields.\n\n\
             For the spec `name:A10 qty:9(5)` the result columns are:\n\n\
             | Name | Type | Description |\n\
             |---|---|---|\n\
             | `name` | VARCHAR | The 10-byte text field decoded from bytes 0–9. |\n\
             | `qty` | BIGINT | The 5-digit zoned integer decoded from bytes 10–14. |\n\n\
             For the single-field spec `id:9(7)` the result is one column:\n\n\
             | Name | Type | Description |\n\
             |---|---|---|\n\
             | `id` | BIGINT | The 7-digit zoned integer decoded from bytes 0–6. |"
                .into(),
        ));
        tags.push((
            "vgi.example_queries".into(),
            r#"[
  {
    "description": "Profile a newline-delimited fixed-width feed of 7-digit ids: count the rows and report the id range.",
    "sql": "SELECT count(*) AS records, min(id) AS min_id, max(id) AS max_id FROM fixed.main.read_fixed('data/large.dat', 'id:9(7)')"
  },
  {
    "description": "Glob several account files (10-char name, 5-digit qty) and total the quantity across every record.",
    "sql": "SELECT sum(qty) AS total_qty FROM fixed.main.read_fixed('data/acct*.dat', 'name:A10 qty:9(5)')"
  }
]"#
            .into(),
        ));
        FunctionMetadata {
            description: "Read a fixed-width file (template / JSON / copybook spec) into rows"
                .into(),
            tags,
            // Opt in to projection pushdown: DuckDB then sends only the columns
            // the query needs, the SDK narrows `producer`'s `output_schema` to
            // them, and the producer materializes Arrow arrays for just those
            // columns (records are still decoded in full — fixed-width offsets
            // are sequential and OCCURS DEPENDING ON reads earlier fields — so
            // the win is in array building, not skipping decode).
            projection_pushdown: true,
            ..Default::default()
        }
    }

    fn argument_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::const_arg(
                "path",
                0,
                "any",
                "Path(s) to the fixed-width file(s) to read: a single path, or a list of paths \
                 to read several files in one call (their rows are concatenated in \
                 order). A path may be a glob (e.g. 'data/*.dat') to scan matching files in sorted \
                 order, or a cloud URL: 's3://bucket/key' (AWS S3, or R2/MinIO/GCS-HMAC via a \
                 `CREATE SECRET (TYPE s3, …, ENDPOINT …)`) or 'https://host/file'. Credentials \
                 come from the matching DuckDB secret, resolved per path scope — so a list \
                 spanning several buckets picks the right secret for each.",
            ),
            ArgSpec::const_arg(
                "spec",
                1,
                "varchar",
                "The record layout to decode each row with: a Perl/Python `unpack` template, a \
                 JSON field list, or a COBOL copybook. Determines the output column names and \
                 types; format is auto-detected unless `format` is given.",
            ),
            ArgSpec::const_arg(
                "format",
                -1,
                "varchar",
                "Force how `spec` is interpreted: 'template', 'json', or 'copybook'. Omit to \
                 auto-detect.",
            )
            .with_choices(options::FORMAT_CHOICES),
            ArgSpec::const_arg(
                "encoding",
                -1,
                "varchar",
                "Byte encoding of the file: 'ascii' (the default) or 'ebcdic' (CP037).",
            )
            .with_choices(options::ENCODING_CHOICES)
            .with_default("ascii"),
            ArgSpec::const_arg(
                "framing",
                -1,
                "varchar",
                "How records are delimited in the file: 'newline' (the default), 'fixed' \
                 (back-to-back records of equal length), 'rdw', or 'rdw_blocked'.",
            )
            .with_choices(options::FRAMING_CHOICES)
            .with_default("newline"),
            ArgSpec::const_arg(
                "record_length",
                -1,
                "int64",
                "The length of each record in BYTES. Used only for 'fixed' framing (back-to-back \
                 equal-length records); ignored for the other framings. Defaults to the length \
                 implied by the layout `spec`.",
            ),
            ArgSpec::const_arg(
                "compression",
                -1,
                "varchar",
                "Input compression: 'auto' (the default — detect gzip/zstd from the file's magic \
                 bytes, else read raw), 'none', 'gzip', or 'zstd'. Applies to local and cloud \
                 paths alike; decompression happens before framing/decoding.",
            )
            .with_choices(options::COMPRESSION_CHOICES)
            .with_default("auto"),
            ArgSpec::const_arg(
                "max_decompressed_bytes",
                -1,
                "int64",
                "Safety cap on total DECOMPRESSED bytes per file (a decompression-bomb backstop; \
                 default 16 GiB). Only applies to gzip/zstd input — uncompressed files are \
                 unbounded. Raise it to read a legitimately huge compressed file.",
            ),
        ]
        .into_iter()
        .chain(options::cloud_arg_specs())
        .collect()
    }

    fn secret_lookups(&self, params: &BindParams) -> Vec<SecretLookup> {
        // Request the matching DuckDB secret per distinct remote scope — so a
        // call spanning several s3 buckets resolves the right secret for each.
        // http(s):// and local paths need none. Best-effort: a malformed path is
        // surfaced later in on_bind.
        match options::paths(&params.arguments, 0) {
            Ok(paths) => cloud::secret_lookups(&paths),
            Err(_) => Vec::new(),
        }
    }

    fn on_bind(&self, params: &BindParams) -> Result<BindResponse> {
        let layout = options::layout(&params.arguments, 1)?;
        let paths = options::paths(&params.arguments, 0)?;
        // Validate local paths early (mirrors the native "File not found").
        // Remote paths are validated lazily at producer time — no network call
        // (or resolved secrets) is needed just to compute the output schema.
        for p in &paths {
            if !cloud::is_remote(p) {
                crate::table::resolve_local(p)?;
            }
        }
        Ok(BindResponse {
            output_schema: output_schema(&layout)?,
            opaque_data: Vec::new(),
        })
    }

    fn producer(&self, params: &ProcessParams) -> Result<Box<dyn TableProducer>> {
        let layout = options::layout(&params.arguments, 1)?;
        let enc = options::encoding(&params.arguments)?;
        let framing = options::framing(&params.arguments)?;
        let rec_len = record_length(&params.arguments, &layout);
        let compression = options::compression(&params.arguments)?;
        let limits = options::read_limits(&params.arguments)?;
        // Reject `fixed` framing for a variable-length layout up front (same
        // guard the eager `COPY … FROM` path applies).
        crate::reader::check_variable_framing(&layout, framing)?;

        let paths = options::paths(&params.arguments, 0)?;
        let overrides = options::cloud_overrides(&params.arguments);
        // Resolve each path (glob/list) to concrete locations, in order, then to
        // openable sources (remote stores are built here but fetched lazily, so
        // a multi-file glob streams one object at a time).
        let mut locations = Vec::new();
        for p in &paths {
            locations.extend(crate::table::resolve_locations(
                p,
                &params.secrets,
                &overrides,
            )?);
        }
        let sources = crate::reader::resolve_sources(&locations, &params.secrets, &overrides)?;

        // The full set of decoded column names, in layout (decode) order. The
        // producer maps each (possibly projection-narrowed / reordered)
        // `output_schema` column to its decoded field BY NAME against this list,
        // so projection pushdown places every value under the right column and
        // only materializes the projected columns. (`params.output_schema` is
        // already narrowed by the SDK when projection is active.)
        let full_names: Vec<String> = output_schema(&layout)?
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();

        // Stream records: one batch is decoded per `next_batch`, so peak memory
        // is ~one batch rather than every decoded row (newline / fixed framing).
        Ok(Box::new(crate::reader::StreamingProducer::new(
            params.output_schema.clone(),
            &full_names,
            layout,
            enc,
            framing,
            rec_len,
            compression,
            limits,
            sources,
        )?))
    }
}
