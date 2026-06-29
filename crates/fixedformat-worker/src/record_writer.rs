//! Shared fixed-width *writing*: turn Arrow record batches into framed record
//! bytes. Used by both `write_fixed` (the `(FROM rel)` table-buffering sink) and
//! the `COPY ... TO` writer (`copy_to`), so the encode + framing logic lives in
//! exactly one place.
//!
//! Each input row's columns are matched **by name** to the layout fields,
//! encoded to record bytes per the [`Layout`], then framed per the [`Framing`]
//! mode into the final file body.

use arrow_array::RecordBatch;
use fixedformat_core::encode::encode_record;
use fixedformat_core::framing::Framing;
use fixedformat_core::{Encoding, Layout, Value};
use vgi_rpc::{Result, RpcError};

use crate::value_in::value_at;

fn ve(e: impl std::fmt::Display) -> RpcError {
    RpcError::value_error(e.to_string())
}

/// Encode every row of a relation batch into record bytes, appending to `out`.
/// Columns are matched to layout fields by name.
pub fn encode_batch(
    batch: &RecordBatch,
    layout: &Layout,
    enc: Encoding,
    out: &mut Vec<Vec<u8>>,
) -> Result<()> {
    let schema = batch.schema();
    for row in 0..batch.num_rows() {
        let mut pairs: Vec<(String, Value)> = Vec::with_capacity(batch.num_columns());
        for (c, field) in schema.fields().iter().enumerate() {
            pairs.push((field.name().clone(), value_at(batch.column(c), row)?));
        }
        out.push(encode_record(layout, &pairs, enc).map_err(ve)?);
    }
    Ok(())
}

/// Frame the encoded records into the final file body per the `framing` mode.
pub fn assemble(records: &[Vec<u8>], framing: Framing) -> Vec<u8> {
    let mut body = Vec::new();
    match framing {
        Framing::Newline => {
            for rec in records {
                body.extend_from_slice(rec);
                body.push(b'\n');
            }
        }
        Framing::Fixed => {
            for rec in records {
                body.extend_from_slice(rec);
            }
        }
        Framing::Rdw => {
            for rec in records {
                push_descriptor(&mut body, rec.len() + 4);
                body.extend_from_slice(rec);
            }
        }
        Framing::RdwBlocked => {
            // One block wrapping all RDW-framed records.
            let block_len: usize = 4 + records.iter().map(|r| r.len() + 4).sum::<usize>();
            push_descriptor(&mut body, block_len);
            for rec in records {
                push_descriptor(&mut body, rec.len() + 4);
                body.extend_from_slice(rec);
            }
        }
    }
    body
}

/// Write a 4-byte descriptor word (big-endian length, then two zero bytes).
fn push_descriptor(body: &mut Vec<u8>, len: usize) {
    let len = len as u16;
    body.extend_from_slice(&len.to_be_bytes());
    body.extend_from_slice(&[0, 0]);
}
