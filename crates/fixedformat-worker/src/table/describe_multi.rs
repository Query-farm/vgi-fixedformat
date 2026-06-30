//! `describe_multi(spec)` — introspect a **multi-record** spec without reading
//! data: one row per (record type, field), so you can see how every variant's
//! layout resolves (DuckDB type, byte offset, width, OCCURS info) before running
//! `read_multi` / `unpack_multi`. The multi-record counterpart of `describe_fixed`.

use std::sync::Arc;

use arrow_array::builder::{Int64Builder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field as ArrowField, Schema, SchemaRef};
use fixedformat_core::describe::{describe, FieldDesc};
use vgi::table_function::{TableFunction, TableProducer};
use vgi::{ArgSpec, BindParams, BindResponse, FunctionMetadata, ProcessParams};
use vgi_rpc::{OutputCollector, Result, RpcError};

pub struct DescribeMulti;

/// The fixed output schema: `describe_fixed`'s columns plus a leading
/// `record_type` (the variant's discriminator tag).
fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        ArrowField::new("record_type", DataType::Utf8, false),
        ArrowField::new("path", DataType::Utf8, false),
        ArrowField::new("depth", DataType::Int64, false),
        ArrowField::new("kind", DataType::Utf8, false),
        ArrowField::new("sql_type", DataType::Utf8, false),
        ArrowField::new("byte_offset", DataType::Int64, false),
        ArrowField::new("width", DataType::Int64, false),
        ArrowField::new("occurs", DataType::Int64, true),
        ArrowField::new("depending_on", DataType::Utf8, true),
    ]))
}

impl TableFunction for DescribeMulti {
    fn name(&self) -> &str {
        "describe_multi"
    }

    fn metadata(&self) -> FunctionMetadata {
        let mut tags = crate::meta::object_tags(
            "Describe Multi-Record-Type Spec",
            "Introspect a multi-record `spec` without reading any data: returns one row per (record \
             type, field), describing how every variant's layout resolves. The leading \
             `record_type` column is the discriminator tag; the remaining columns mirror \
             describe_fixed — `path` (dotted field path), `depth`, `kind` (codec label), `sql_type` \
             (the DuckDB type the field maps to), `byte_offset` and `width`, `occurs` (OCCURS \
             maximum, else NULL), and `depending_on` (the OCCURS … DEPENDING ON controller, else \
             NULL). The `spec` is the same multi-record JSON object as read_multi (a `discriminator` \
             plus a `records` map of tag → field list). Use it to debug a multi-record layout or \
             document each record type before running read_multi / unpack_multi.",
            "Describe how a multi-record `spec` resolves — one row per (record type, field) with its \
             dotted path, codec kind, DuckDB type, byte offset, width, and OCCURS info. Reads no \
             data. The multi-record counterpart of describe_fixed.",
            "describe multi, introspect, multi-record, heterogeneous, discriminator, record type, \
             layout, schema, fields, offsets, debug spec",
        );
        tags.push((
            "vgi.result_columns_md".into(),
            "A **fixed** result schema — one row per (record type, field):\n\n\
             | column | type | description |\n\
             |---|---|---|\n\
             | `record_type` | VARCHAR | The discriminator tag of the record type this field \
             belongs to. |\n\
             | `path` | VARCHAR | Dotted field path within that record type. |\n\
             | `depth` | BIGINT | Nesting level (0 at the top). |\n\
             | `kind` | VARCHAR | Codec label (e.g. `text`, `int32 LE`, `comp-3`). |\n\
             | `sql_type` | VARCHAR | The DuckDB column type the field maps to. |\n\
             | `byte_offset` | BIGINT | Static byte position within the record. |\n\
             | `width` | BIGINT | Per-occurrence width in bytes. |\n\
             | `occurs` | BIGINT | OCCURS maximum, else NULL. |\n\
             | `depending_on` | VARCHAR | OCCURS … DEPENDING ON controller, else NULL. |"
                .into(),
        ));
        FunctionMetadata {
            description:
                "Describe a multi-record spec (each record type's fields) without reading \
                          data"
                    .into(),
            tags,
            ..Default::default()
        }
    }

    fn argument_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::const_arg(
            "spec",
            0,
            "varchar",
            "The multi-record JSON layout to describe: a `discriminator` ({offset, width}) plus a \
             `records` map of record-type tag → JSON field list (same as read_multi).",
        )]
    }

    fn on_bind(&self, params: &BindParams) -> Result<BindResponse> {
        // The multi-record spec is the const arg at position 0 here, but
        // `multi_layout` reads position 1 (read_multi's path is at 0). Parse
        // directly so spec errors surface at bind time.
        let spec = params.arguments.const_str(0).ok_or_else(|| {
            RpcError::value_error("a multi-record layout spec string is required")
        })?;
        fixedformat_core::multirecord::MultiLayout::parse(&spec)
            .map_err(|e| RpcError::value_error(e.to_string()))?;
        Ok(BindResponse {
            output_schema: schema(),
            opaque_data: Vec::new(),
        })
    }

    fn producer(&self, params: &ProcessParams) -> Result<Box<dyn TableProducer>> {
        let spec = params.arguments.const_str(0).ok_or_else(|| {
            RpcError::value_error("a multi-record layout spec string is required")
        })?;
        let ml = fixedformat_core::multirecord::MultiLayout::parse(&spec)
            .map_err(|e| RpcError::value_error(e.to_string()))?;
        // Flatten: one (tag, field-desc) per field across every variant, in
        // declaration order.
        let mut rows: Vec<(String, FieldDesc)> = Vec::new();
        for (tag, layout) in &ml.variants {
            for fd in describe(layout) {
                rows.push((tag.clone(), fd));
            }
        }
        Ok(Box::new(DescribeMultiProducer {
            schema: params.output_schema.clone(),
            rows: Some(rows),
        }))
    }
}

struct DescribeMultiProducer {
    schema: SchemaRef,
    rows: Option<Vec<(String, FieldDesc)>>,
}

impl TableProducer for DescribeMultiProducer {
    fn next_batch(&mut self, _out: &mut OutputCollector) -> Result<Option<RecordBatch>> {
        let Some(rows) = self.rows.take() else {
            return Ok(None);
        };

        let mut record_type = StringBuilder::new();
        let mut path = StringBuilder::new();
        let mut depth = Int64Builder::new();
        let mut kind = StringBuilder::new();
        let mut sql_type = StringBuilder::new();
        let mut offset = Int64Builder::new();
        let mut width = Int64Builder::new();
        let mut occurs = Int64Builder::new();
        let mut depending_on = StringBuilder::new();

        for (tag, r) in &rows {
            record_type.append_value(tag);
            path.append_value(&r.path);
            depth.append_value(r.depth as i64);
            kind.append_value(&r.kind);
            sql_type.append_value(&r.sql_type);
            offset.append_value(r.offset as i64);
            width.append_value(r.width as i64);
            match r.occurs {
                Some(n) => occurs.append_value(n as i64),
                None => occurs.append_null(),
            }
            match &r.depending_on {
                Some(c) => depending_on.append_value(c),
                None => depending_on.append_null(),
            }
        }

        let columns: Vec<ArrayRef> = vec![
            Arc::new(record_type.finish()),
            Arc::new(path.finish()),
            Arc::new(depth.finish()),
            Arc::new(kind.finish()),
            Arc::new(sql_type.finish()),
            Arc::new(offset.finish()),
            Arc::new(width.finish()),
            Arc::new(occurs.finish()),
            Arc::new(depending_on.finish()),
        ];
        Ok(Some(
            RecordBatch::try_new(self.schema.clone(), columns)
                .map_err(|e| RpcError::runtime_error(e.to_string()))?,
        ))
    }
}
