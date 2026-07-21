//! wasm32-only pieces of the cloud stack.
//!
//! Natively, `object_store` brings its own transport (`reqwest`/`rustls`) and
//! crypto (`aws-lc-rs`). Neither compiles to wasm — reqwest needs sockets and
//! aws-lc-rs is C/asm — so the wasm build depends on `object_store`'s
//! *`-base`* features, which deliberately ship neither, and supplies both here:
//!
//! - [`http`] — an [`object_store::client::HttpService`] over the browser's
//!   synchronous `XMLHttpRequest`, reached through one `vgi_http_send` import
//!   implemented in the emscripten `--js-library` (see `wasm/vgi_http_lib.js`).
//! - [`crypto`] — an [`object_store::client::CryptoProvider`] over pure-Rust
//!   `sha2`/`hmac`, which is what makes SigV4 request signing work.
//!
//! `cloud::build_store` wires both into the S3/HTTP builders on this target.
//! Everything above that seam — signing, XML list pagination, retry, the
//! `ObjectStore` API itself — is the same object_store code the native build
//! runs.

pub mod crypto;
pub mod http;
