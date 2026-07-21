//! Pure-Rust CryptoProvider for object_store's SigV4 signing.
//!
//! `aws-base` ships no crypto provider — the bundled ones (`aws-lc-rs`, `ring`)
//! are C/asm and don't build for wasm. sha2/hmac are pure Rust and do.
use std::sync::Arc;

use hmac::{Hmac, Mac};
use object_store::client::{
    CryptoProvider, DigestAlgorithm, DigestContext, HmacContext, Signer, SigningAlgorithm,
};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct RustCrypto;

pub fn provider() -> Arc<dyn CryptoProvider> {
    Arc::new(RustCrypto)
}

struct Sha256Ctx {
    inner: Sha256,
    out: [u8; 32],
}

impl DigestContext for Sha256Ctx {
    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }
    fn finish(&mut self) -> object_store::Result<&[u8]> {
        self.out = std::mem::take(&mut self.inner).finalize().into();
        Ok(&self.out)
    }
}

struct HmacCtx {
    inner: Hmac<Sha256>,
    out: [u8; 32],
}

impl HmacContext for HmacCtx {
    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }
    fn finish(&mut self) -> object_store::Result<&[u8]> {
        let mac = std::mem::replace(
            &mut self.inner,
            Hmac::<Sha256>::new_from_slice(&[]).expect("hmac accepts any key length"),
        );
        self.out = mac.finalize().into_bytes().into();
        Ok(&self.out)
    }
}

impl CryptoProvider for RustCrypto {
    fn digest(&self, algorithm: DigestAlgorithm) -> object_store::Result<Box<dyn DigestContext>> {
        match algorithm {
            DigestAlgorithm::Sha256 => Ok(Box::new(Sha256Ctx {
                inner: Sha256::new(),
                out: [0; 32],
            })),
            _ => Err(object_store::Error::NotSupported {
                source: "unsupported digest algorithm".into(),
            }),
        }
    }

    fn hmac(
        &self,
        algorithm: DigestAlgorithm,
        secret: &[u8],
    ) -> object_store::Result<Box<dyn HmacContext>> {
        match algorithm {
            DigestAlgorithm::Sha256 => Ok(Box::new(HmacCtx {
                inner: Hmac::<Sha256>::new_from_slice(secret).expect("hmac accepts any key length"),
                out: [0; 32],
            })),
            _ => Err(object_store::Error::NotSupported {
                source: "unsupported hmac algorithm".into(),
            }),
        }
    }

    fn sign(
        &self,
        _algorithm: SigningAlgorithm,
        _pem: &[u8],
    ) -> object_store::Result<Box<dyn Signer>> {
        // RS256 is a GCP service-account concern; S3/R2 never reaches this.
        Err(object_store::Error::NotSupported {
            source: "RS256 signing not supported (S3/R2 does not need it)".into(),
        })
    }
}
