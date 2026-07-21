# Browser (WASM) build of the fixedformat worker

The same worker the native binary serves, compiled to
`wasm32-unknown-emscripten` and served over DuckDB-WASM's SharedArrayBuffer
channel instead of stdio. Every SQL function is registered identically —
`build_worker()` in `fixedformat-worker` is shared by both entrypoints, so the
two builds cannot drift.

```sh
./wasm/build.sh                 # → wasm/dist/fixedformat_worker.{js,wasm}
EMSDK_DIR=/path/to/emsdk ./wasm/build.sh
```

Requires **emsdk** and a **nightly** toolchain (`-Z build-std` is mandatory: the
pthread build recompiles `compiler_builtins` with atomics).

## Wiring it up

The build emits a MODULARIZE'd module (`EXPORT_NAME=FixedFormatWorker`) that a
Web Worker loads with `importScripts`. Attach it from SQL as
`LOCATION 'worker:/path/to/your-boot.js'`.

Your boot script **must not run the module factory at load time.** Emscripten
spawns pthread helpers by re-instantiating the same script, so unguarded work
recurses: each helper builds another module and spawns another pool. Gate
everything behind an explicit message (this is what
`vgi-worker-boot.js` in the vgi repo does):

```js
importScripts('fixedformat_worker.js');
let started = false;
self.addEventListener('message', async (e) => {
  if (e.data?.type !== 'vgi-init' || started) return;
  started = true;
  globalThis.__vgiBuf = e.data.buffer;      // DuckDB's delivered SAB
  globalThis.__vgiBase = e.data.offset;
  const M = await FixedFormatWorker({});
  M._fixedformat_wasm_init();                // MUST run before serving
  self.postMessage({ type: 'vgi-ready' });
  M._fixedformat_serve_pool(nSlots);         // one serve thread per ring slot
});
```

`fixedformat_wasm_init()` selects the in-memory shared-storage backend. Without
it the buffering functions (`write_fixed`, `write_multi`, `COPY … TO`) try to use
a filesystem temp dir that does not exist under MEMFS.

## Exports

| Symbol | Purpose |
| --- | --- |
| `fixedformat_wasm_init()` | Select in-memory shared storage. Call once, before serving. |
| `fixedformat_serve_pool(n)` | Spawn one serve thread per ring slot. `n` ≤ `PTHREAD_POOL_SIZE` (4). |
| `fixedformat_serve_sab_slot(slot)` | Serve a single request lifecycle on one slot. |
| `fixedformat_wasm_mount_opfs()` | Mount OPFS at `/opfs`. Auto-mounted on first serve; not callable from the main thread. |
| `fixedformat_wasm_selftest_http(ptr, len, n)` | Deployment health check — see below. |
| `fixedformat_wasm_opfs_selftest()` | OPFS round-trip check. |
| `fixedformat_wasm_bench_opfs(...)` | Load-test harness. Only with `FIXEDFORMAT_BENCH=1`. |

## The SAB ring js-library is not vendored

`build.sh` needs `VGI_WORKER_LIB` pointing at `vgi_worker_lib.js` from the vgi
extension repo (`test/support/wasm-worker/`), defaulting to
`../vgi/test/support/wasm-worker/vgi_worker_lib.js`. That file implements the
ring ops and must stay byte-exact with `vgi_sab_abi.hpp`; a stale vendored copy
would fail as data corruption rather than a clear error, so we link the
canonical one.

`wasm/vgi_http_lib.js` *is* ours — it implements the single `vgi_http_send`
import backing cloud reads.

## How the cloud path differs from native

Natively, `object_store` brings `reqwest`/`rustls` for transport and `aws-lc-rs`
for crypto. Neither compiles to wasm, so the wasm build uses object_store's
`aws-base`/`http-base` features — the same S3 code *without* a bundled transport
or crypto provider — and supplies both:

- **Transport** (`src/wasm/http.rs`) — an `HttpService` over synchronous
  `XMLHttpRequest`. Legal in a Worker realm; it throws `InvalidAccessError` on a
  document, which is another reason the module must live in a Worker.
- **Crypto** (`src/wasm/crypto.rs`) — a `CryptoProvider` over pure-Rust
  `sha2`/`hmac`. This is what makes SigV4 signing work; without it every signed
  request fails with *"Must enable aws-lc-rs, ring, or specify custom
  CryptoProvider"*.

Everything above that seam — signing, XML list pagination, retry, `ObjectStore`
itself — is the same object_store code the native build runs. `zstd` similarly
swaps the C library for pure-Rust `ruzstd`; both read and write standard frames,
so files are interchangeable.

## OPFS (persistent browser storage)

The build links WASMFS with the OPFS backend, and the worker mounts it at
`/opfs` on first serve. Because that exposes OPFS through ordinary POSIX file
descriptors, **every existing local-path feature works unchanged** — no
wasm-specific code paths:

```sql
SELECT * FROM fixed.main.read_fixed('/opfs/data/*.dat', 'id:9(8) sku:A12');
CALL fixed.main.write_fixed((FROM t), '/opfs/out.dat', 'id:9(8) sku:A12');
COPY t FROM '/opfs/in.dat' (FORMAT 'fixed.fixed', spec '…');
```

Files persist across page loads (verified: written on one load, read back on the
next). Everything else stays MEMFS and is lost on reload.

**Mounting happens on a serve thread, never at init.** OPFS operations are
proxied to the thread running the JS event loop, so mounting *from* that thread
deadlocks. `fixedformat_serve_sab_slot` mounts via a `Once`, so the first serve
thread does it and the rest wait. `fixedformat_wasm_mount_opfs` is exported for
hosts that want to mount eagerly — same constraint applies: not from the main
thread.

A failed mount is logged and tolerated: only `/opfs/…` paths become unavailable.

## Load testing

`FIXEDFORMAT_BENCH=1 ./wasm/build.sh` adds `fixedformat_wasm_bench_opfs(files,
records_per_file, out, out_cap)`, which generates fixed-width files under
`/opfs/bench/` and scans them back through the production pipeline
(`fixedformat_worker::bench_scan_local` — the same framing, decode, and Arrow
construction `read_fixed` uses), returning JSON timings. Off by default so
production artifacts don't carry benchmark scaffolding.

Measured in Chrome on an M-series Mac, 61-byte records, spec `id:9(8) sku:A12
name:A24 qty:9(6) amount:9(9)V99`, newline-framed, read from OPFS. Native numbers
come from the same `bench_scan_local` at the same opt-level, measured with the
browser idle (running both at once skews native badly).

**Parallel scan** (`fixedformat_wasm_bench_parallel`; N threads over disjoint
file sets, as a fanned-out scan would):

| Threads | wasm rows/s | native rows/s | wasm scaling | native scaling |
| --- | --- | --- | --- | --- |
| 1 | 486k | 1.62M | 1.00× | 1.00× |
| 2 | 818k | 3.08M | 1.68× | 1.90× |
| 4 | 1.33M | 5.64M | 2.73× | 3.48× |

So wasm is **~3.3× slower than native single-threaded and ~4.2× at 4 threads**,
and its parallel scaling is real but weaker (2.73× vs 3.48× on 4 threads).

**Single-threaded scan** across file counts (100k rows unless noted):

| Shape | Rows | Scan | Rows/s |
| --- | --- | --- | --- |
| 1 × 100k | 100,000 | 239 ms | 418k |
| 10 × 10k | 100,000 | 225 ms | 444k |
| 100 × 1k | 100,000 | 271 ms | 369k |
| 200 × 5k | 1,000,000 | 3,052 ms | 328k |

(Those were taken at opt-level 2; opt-level 3 is now the default and raises them
by ~30%.) Per-file open overhead is small — ~15% going from 1 file to 100.

### Cloud (S3/R2) scans are latency-bound, and parallelism pays much more

Measured against a real Cloudflare R2 bucket, 5 objects × 10k records per thread,
each thread-count reading its own prefix (reusing objects lets the browser HTTP
cache fake a speedup — the first attempt showed `per_thread_ms: [64, 1270]`):

| Threads | Rows | Wall | Rows/s | vs 1 thread | overlap |
| --- | --- | --- | --- | --- | --- |
| 1 | 50,000 | 1,395 ms | 35.8k | 1.00× | 1.00 |
| 2 | 100,000 | 1,177 ms | 85.0k | 2.37× | 1.84 |
| 4 | 200,000 | 1,249 ms | 160k | 4.47× | 3.57 |

(Before `range_reader` stopped issuing a redundant `head` per object these were
26.7k / 56.3k / 122k — see "Cloud reads are round-trip bound" below.)

Two things follow:

- **Concurrent synchronous XHRs on separate serve threads genuinely overlap** —
  `overlap` (Σ thread time ÷ wall) reaches 3.67 at 4 threads, and the browser's
  ~6-connections-per-origin limit does not bind at this width.
- **Scaling is super-linear** (4.58× on 4 threads) because each thread is waiting
  on the network, not the CPU. Wall time barely moves (1,872 → 1,634 ms) while
  doing 4× the work.

At 35.8k rows/s single-threaded, cloud reads are ~14× slower than the same scan
from OPFS (486k). **For cloud-backed scans the decode cost is irrelevant** — the
wasm-vs-native CPU gap below does not matter, and fanning out is by far the
highest-leverage thing you can do.

### Cloud reads are round-trip bound, not bandwidth bound

Measured against R2 from a laptop, per 620 KB object:

| Request | Time |
| --- | --- |
| `HEAD` | ~120 ms |
| `GET` (TTFB ~150 ms + ~85 ms transfer) | ~240 ms |
| single-connection bandwidth | 2.4–2.8 MB/s |

`range_reader` originally issued **both** per object — ~360 ms × 5 objects
sequentially ≈ 1,800 ms, matching the measured 1,872 ms almost exactly. The
`head` existed only to learn the object size, which `GetResult::meta` already
carries, so it was pure overhead for anything fitting in one 8 MiB chunk (~33% of
a small-object scan). It now fetches the first chunk with `get_opts` and takes the
size from that response: **+34% single-threaded, +51% at 2 threads.** This is
shared code, so native `s3://` scans get the same win.

One wrinkle: a zero-length object cannot satisfy any range request (HTTP 416), so
a failed *first* chunk falls back to `head` and reports EOF when the size is 0.
That costs an extra request only for empty objects, and is covered by a
scan-an-empty-object check.

Within a thread, requests remain strictly serialized — synchronous XHR cannot
pipeline. Parallelism is the only lever, which is why scaling is super-linear.

### Two build flags that are load-bearing

**`-sMALLOC=mimalloc`.** With emscripten's default `dlmalloc` this workload
*anti-scales* — aggregate throughput FALLS as threads are added:

| Threads | dlmalloc | mimalloc |
| --- | --- | --- |
| 1 | 204k rows/s | 397k rows/s |
| 2 | 126k rows/s | 508k rows/s |
| 4 | 63k rows/s | 641k rows/s |

dlmalloc serializes every allocation on one global lock and the decode path
allocates per field per row, so serve threads convoy — each ran ~13× slower at 4
threads. `MALLOC=dlmalloc ./wasm/build.sh` reproduces it.

**`-C opt-level=3` / `-O3`**, overriding the workspace's `opt-level = 2` for this
target only: worth ~2× on the parallel scan (663k → 1.33M rows/s at 4 threads).
`+simd128` was measured separately and made **no** difference, so it is
deliberately not enabled — it would add a browser-compatibility constraint for
nothing.

Dead ends worth recording: raising `INITIAL_MEMORY` to 1 GB made things *worse*
(never memory growth), and rewriting the benchmark from collect-all to streaming
batches changed nothing (never row accumulation).

### Why wasm is ~3.3× slower than native

It is **not** any particular operation. Holding the record bytes identical and
varying only how they decode:

| Spec | wasm | native | ratio |
| --- | --- | --- | --- |
| `amount` as DECIMAL (i128) | 488k rows/s | 1.60M | 3.3× |
| `amount` as BIGINT (i64) | 499k rows/s | 1.64M | 3.3× |
| every field text (no numeric parsing) | 554k rows/s | 1.79M | 3.2× |

The ratio is flat. 128-bit decimal arithmetic — the obvious suspect, since wasm
has no add-with-carry — costs only ~2%, and removing numeric parsing entirely
moves ~10%. The gap is uniform, not a hot spot. Candidates measured and
**eliminated**:

| Hypothesis | Test | Result |
| --- | --- | --- |
| Missing SIMD | build with `+simd128` | **no change** (486k/818k/1333k vs 486k/818k/1328k) |
| i128 decimal math | DECIMAL vs BIGINT spec | ~2% |
| Numeric parsing | numeric vs all-text spec | ~10% |
| JIT tier-up (Liftoff → TurboFan) | same scan ×8 in one process | **no warmup curve**: 523, 579, 560, 595, 567, 566, 562, 537 ms — the first iteration is the *fastest* |
| Allocator contention | `MALLOC=mimalloc` | large, **fixed** (see below) |
| Codegen opt level | `-C opt-level=3` | ~2×, **fixed** (see below) |

Note there is no `target-cpu` knob to miss: LLVM compiles to wasm, a portable
stack-machine ISA with no CPU model, and the browser engine then JITs wasm →
machine code. The final instruction selection is V8's, not LLVM's aarch64
backend — which is the most likely remaining explanation, but it is **not
attributed**; confirming it needs profiling, not more guessing.

### OPFS degrades under repeated directory churn

A benchmark that did `remove_dir_all` + regenerate on each invocation appeared to
**hang** on its fifth run. Running the same spec first completed normally in
451 ms, so it was the churn, not the workload. If a workload repeatedly deletes
and recreates OPFS trees, expect degradation.

## Constraints you should know about

**S3/R2 buckets need a CORS policy**, or ranged reads fail confusingly: the
request *succeeds* with a 200/206 and correct bytes, but `Content-Range` is
stripped and invisible to the worker. Allow `Range`, `Authorization`,
`x-amz-date`, `x-amz-content-sha256` as request headers and expose
`Content-Range`, `Content-Length`, `ETag`. R2 uses Cloudflare's schema, not the
S3-style JSON:

```json
{"rules": [{
  "allowed": {"origins": ["https://yourapp.example"],
              "methods": ["GET", "HEAD"],
              "headers": ["range", "authorization", "x-amz-date", "x-amz-content-sha256"]},
  "exposeHeaders": ["Content-Range", "Content-Length", "ETag"],
  "maxAgeSeconds": 3600
}]}
```

**Credentials reach the end user.** A browser worker signs requests in the
user's own process, so any key it holds is readable by the page. Prefer
presigned URLs or a short-lived token broker over long-lived keys.

**The SSRF guard is disabled on wasm** (`cloud::guard_host`). It exists to stop a
*server-side* worker being aimed at cloud metadata or RFC-1918 hosts. In the
browser the request comes from the user's own machine and the page could issue
the same `fetch()` itself, so the guard adds nothing while breaking legitimate
local/intranet reads. The real boundary is the browser's same-origin policy.

**Sync XHR blocks its serve thread** for the duration of a fetch, and the pool is
4. Concurrency comes from one fanned-out scan, not many concurrent queries.

## Health check

`fixedformat_wasm_selftest_http` runs the production path — `cloud::build_store`
with the XHR connector, then a ranged streaming read — against an
`http(s)://` URL, and logs *why* it failed to stderr (visible via the module's
`printErr`). CORS misconfiguration is the usual cause and is otherwise hard to
recognize from a failing scan.

Returns bytes read, or: `-1` bad UTF-8, `-2` classify, `-3` not remote,
`-4` store construction, `-5` HEAD/range-reader, `-6` read.

## Tuning notes

`STACK_SIZE=1MB` / `DEFAULT_PTHREAD_STACK_SIZE=2MB` are deliberate: emscripten's
64 KiB default overflows here (object_store + tokio + the Arrow encoders nest
deeply), and the overflow surfaces as a bare `RuntimeError: memory access out of
bounds` with no console output. `INITIAL_MEMORY=64MB` must cover the main stack
plus every pthread stack up front, or startup stalls growing shared memory while
the pool spawns.
