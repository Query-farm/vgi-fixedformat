//! zstd encode/decode behind one interface, with a target-specific backend.
//!
//! Native builds use the `zstd` crate (bindings to the reference C library) —
//! fastest, and what every existing test is calibrated against. `wasm32` builds
//! use [`ruzstd`], a pure-Rust implementation, because `zstd-sys` compiles C and
//! there is no C toolchain for the wasm targets we ship.
//!
//! Both backends read and write the standard zstd frame format, so a file
//! written by one is readable by the other — the SQLLogic fixtures are
//! byte-compatible across targets.

use crate::Error;
use std::io::Read;

/// Cap on the zstd window size a frame may request, bounding the decoder's
/// allocation for a hostile input. Only the C backend exposes this knob;
/// `ruzstd` allocates per frame as it goes and has no equivalent setting.
#[cfg(not(target_arch = "wasm32"))]
const ZSTD_WINDOW_LOG_MAX: u32 = 27; // 128 MiB

/// A streaming zstd decoder over `reader`.
#[cfg(not(target_arch = "wasm32"))]
pub fn decoder<'a, R: std::io::BufRead + Send + 'a>(
    reader: R,
) -> Result<Box<dyn Read + Send + 'a>, Error> {
    let mut dec = zstd::stream::read::Decoder::with_buffer(reader)
        .map_err(|e| Error(format!("zstd decode: {e}")))?;
    dec.window_log_max(ZSTD_WINDOW_LOG_MAX)
        .map_err(|e| Error(format!("zstd decode: {e}")))?;
    Ok(Box::new(dec))
}

/// A streaming zstd decoder over `reader`.
#[cfg(target_arch = "wasm32")]
pub fn decoder<'a, R: std::io::BufRead + Send + 'a>(
    reader: R,
) -> Result<Box<dyn Read + Send + 'a>, Error> {
    let dec = ruzstd::decoding::StreamingDecoder::new(reader)
        .map_err(|e| Error(format!("zstd decode: {e}")))?;
    Ok(Box::new(dec))
}

/// Decompress a whole zstd buffer.
pub fn decode_all(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    decoder(data)?
        .read_to_end(&mut out)
        .map_err(|e| Error(format!("zstd decode: {e}")))?;
    Ok(out)
}

/// Compress a whole buffer to a zstd frame at the default level.
#[cfg(not(target_arch = "wasm32"))]
pub fn encode_all(data: &[u8]) -> Result<Vec<u8>, Error> {
    zstd::stream::encode_all(data, 0).map_err(|e| Error(format!("zstd encode: {e}")))
}

/// Compress a whole buffer to a zstd frame at the default level.
#[cfg(target_arch = "wasm32")]
pub fn encode_all(data: &[u8]) -> Result<Vec<u8>, Error> {
    // ruzstd's compressor is infallible (it falls back to storing raw blocks),
    // so there is no error path to map here.
    Ok(ruzstd::encoding::compress_to_vec(
        data,
        ruzstd::encoding::CompressionLevel::Default,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(decode_all(&encode_all(&data).unwrap()).unwrap(), data);
    }

    #[test]
    fn empty_roundtrips() {
        assert_eq!(decode_all(&encode_all(b"").unwrap()).unwrap(), b"");
    }

    #[test]
    fn corrupt_frame_is_an_error() {
        // Valid magic, garbage body — must fail rather than return partial data.
        let mut bad = vec![0x28, 0xb5, 0x2f, 0xfd];
        bad.extend_from_slice(&[0xff; 32]);
        assert!(decode_all(&bad).is_err());
    }
}
