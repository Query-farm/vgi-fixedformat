//! object_store HTTP transport over the browser's synchronous XMLHttpRequest.
use async_trait::async_trait;
use object_store::client::{
    ClientOptions, HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpResponse,
    HttpResponseBody, HttpService,
};

extern "C" {
    /// Returns 0 on success (response blob malloc'd, ptr/len written), -1 on
    /// network/CORS failure. Implemented in the emscripten --js-library.
    fn vgi_http_send(
        req_ptr: *const u8,
        req_len: i32,
        out_ptr: *mut *mut u8,
        out_len: *mut i32,
    ) -> i32;
    fn free(p: *mut u8);
}

#[derive(Debug, Default)]
pub struct XhrConnector;

impl HttpConnector for XhrConnector {
    fn connect(&self, _opts: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(XhrService))
    }
}

#[derive(Debug)]
pub struct XhrService;

fn err(kind: HttpErrorKind, msg: &str) -> HttpError {
    HttpError::new(kind, std::io::Error::other(msg.to_string()))
}

#[async_trait]
impl HttpService for XhrService {
    async fn call(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let (parts, body) = req.into_parts();
        let body = body
            .as_bytes()
            .ok_or_else(|| err(HttpErrorKind::Request, "streaming request body unsupported"))?
            .clone();

        let wire = wire::encode_request(&parts.method, &parts.uri, &parts.headers, &body);

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: i32 = 0;
        let rc =
            unsafe { vgi_http_send(wire.as_ptr(), wire.len() as i32, &mut out_ptr, &mut out_len) };
        if rc != 0 || out_ptr.is_null() {
            return Err(err(HttpErrorKind::Connect, "xhr failed (network or CORS)"));
        }
        let raw = unsafe { std::slice::from_raw_parts(out_ptr, out_len as usize) }.to_vec();
        unsafe { free(out_ptr) };

        let (status, headers, body) =
            wire::decode_response(&raw).map_err(|e| err(HttpErrorKind::Decode, &e))?;

        let mut b = http::Response::builder().status(status);
        for (k, v) in headers {
            b = b.header(k, v);
        }
        b.body(HttpResponseBody::from(body))
            .map_err(|e| err(HttpErrorKind::Decode, &e.to_string()))
    }
}

mod wire {
    use bytes::Bytes;
    use http::{HeaderMap, Method, Uri};

    fn put(out: &mut Vec<u8>, b: &[u8]) {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }

    pub fn encode_request(m: &Method, u: &Uri, h: &HeaderMap, body: &Bytes) -> Vec<u8> {
        let mut o = Vec::with_capacity(256 + body.len());
        put(&mut o, m.as_str().as_bytes());
        put(&mut o, u.to_string().as_bytes());
        o.extend_from_slice(&(h.len() as u32).to_le_bytes());
        for (k, v) in h.iter() {
            put(&mut o, k.as_str().as_bytes());
            put(&mut o, v.as_bytes());
        }
        put(&mut o, body);
        o
    }

    struct Cur<'a>(&'a [u8], usize);
    impl<'a> Cur<'a> {
        fn u32(&mut self) -> Result<u32, String> {
            let e = self.1 + 4;
            let b = self.0.get(self.1..e).ok_or("short read")?;
            self.1 = e;
            Ok(u32::from_le_bytes(b.try_into().unwrap()))
        }
        fn bytes(&mut self) -> Result<&'a [u8], String> {
            let n = self.u32()? as usize;
            let e = self.1 + n;
            let b = self.0.get(self.1..e).ok_or("short read")?;
            self.1 = e;
            Ok(b)
        }
        fn str(&mut self) -> Result<String, String> {
            String::from_utf8(self.bytes()?.to_vec()).map_err(|e| e.to_string())
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn decode_response(raw: &[u8]) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
        let mut c = Cur(raw, 0);
        let status = c.u32()? as u16;
        let n = c.u32()? as usize;
        let mut hs = Vec::with_capacity(n);
        for _ in 0..n {
            hs.push((c.str()?, c.str()?));
        }
        Ok((status, hs, c.bytes()?.to_vec()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn roundtrip_response() {
            let mut o = Vec::new();
            o.extend_from_slice(&206u32.to_le_bytes());
            o.extend_from_slice(&1u32.to_le_bytes());
            put(&mut o, b"content-range");
            put(&mut o, b"bytes 0-3/9");
            put(&mut o, &[0xC1, 0x00, 0xFF, 0x7F]); // non-UTF8: the whole point
            let (s, h, b) = decode_response(&o).unwrap();
            assert_eq!(s, 206);
            assert_eq!(h[0].1, "bytes 0-3/9");
            assert_eq!(b, vec![0xC1, 0x00, 0xFF, 0x7F]);
        }
    }
}
