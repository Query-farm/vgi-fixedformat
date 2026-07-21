// emscripten --js-library implementing vgi_http_send: one generic synchronous
// HTTP call backing object_store's HttpService.
//
// Request/response are length-prefixed binary blobs (mirror of src/xhr.rs::wire)
// so headers and bodies stay arbitrary bytes. Deliberately NOT responseText —
// that round-trips through UTF-8 and corrupts any non-UTF-8 byte.
addToLibrary({
  $vgiHttp: {
    // NEVER cache a heap view: sync XHR blocks this pthread, and another thread
    // can grow memory meanwhile, detaching the buffer we were holding.
    u8: function () { return new Uint8Array(wasmMemory.buffer); },
    dv: function () { return new DataView(wasmMemory.buffer); },

    decodeRequest: function (buf) {
      var off = 0;
      var dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
      function u32() { var v = dv.getUint32(off, true); off += 4; return v; }
      function bytes() { var n = u32(); var b = buf.subarray(off, off + n); off += n; return b; }
      function str() { return new TextDecoder().decode(bytes()); }
      var method = str();
      var url = str();
      var n = u32();
      var headers = [];
      for (var i = 0; i < n; i++) { headers.push([str(), str()]); }
      // Copy: XHR rejects a SharedArrayBuffer-backed view.
      var body = new Uint8Array(bytes());
      return { method: method, url: url, headers: headers, body: body };
    },

    encodeResponse: function (status, rawHeaders, body) {
      var hs = [];
      (rawHeaders || '').trim().split(/[\r\n]+/).forEach(function (line) {
        if (!line) return;
        var i = line.indexOf(':');
        if (i < 0) return;
        hs.push([line.slice(0, i).trim(), line.slice(i + 1).trim()]);
      });
      var enc = new TextEncoder();
      var parts = [];
      var total = 8; // status + n_headers
      hs.forEach(function (kv) {
        var k = enc.encode(kv[0]), v = enc.encode(kv[1]);
        parts.push(k, v);
        total += 8 + k.length + v.length;
      });
      total += 4 + body.length;

      var out = new Uint8Array(total);
      var dv = new DataView(out.buffer);
      var off = 0;
      dv.setUint32(off, status, true); off += 4;
      dv.setUint32(off, hs.length, true); off += 4;
      parts.forEach(function (p) {
        dv.setUint32(off, p.length, true); off += 4;
        out.set(p, off); off += p.length;
      });
      dv.setUint32(off, body.length, true); off += 4;
      out.set(body, off);
      return out;
    },
  },

  vgi_http_send__deps: ['$vgiHttp', 'malloc'],
  vgi_http_send: function (reqPtr, reqLen, outPtrPtr, outLenPtr) {
    try {
      // .slice → non-shared copy; TextDecoder rejects SAB-backed views.
      var req = vgiHttp.u8().slice(reqPtr, reqPtr + reqLen);
      var r = vgiHttp.decodeRequest(req);

      var xhr = new XMLHttpRequest();
      xhr.open(r.method, r.url, false);      // synchronous
      xhr.responseType = 'arraybuffer';      // legal in a Worker realm
      for (var i = 0; i < r.headers.length; i++) {
        try { xhr.setRequestHeader(r.headers[i][0], r.headers[i][1]); } catch (e) { /* forbidden header */ }
      }
      xhr.send(r.body.length ? r.body : null);

      var body = new Uint8Array(xhr.response || new ArrayBuffer(0));
      var resp = vgiHttp.encodeResponse(xhr.status | 0, xhr.getAllResponseHeaders(), body);

      var p = _malloc(resp.length);          // may grow memory → re-derive views
      if (!p) return -1;
      vgiHttp.u8().set(resp, p);
      var dv = vgiHttp.dv();
      dv.setUint32(outPtrPtr, p, true);
      dv.setInt32(outLenPtr, resp.length, true);
      return 0;
    } catch (e) {
      if (typeof console !== 'undefined') console.error('[vgi_http_send]', e);
      return -1;
    }
  },
});
