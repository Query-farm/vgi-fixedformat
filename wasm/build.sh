#!/usr/bin/env bash
# Build the browser (`worker:`) build of the fixedformat VGI worker:
# fixedformat-wasm compiled to wasm32-unknown-emscripten, linked by emcc with the
# HTTP js-library into a MODULARIZE'd Web Worker module.
#
#   ./wasm/build.sh              → wasm/dist/vgi_worker.{js,wasm}
#   EMSDK_DIR=/path ./wasm/build.sh
#
# Requirements:
#   - emsdk (emcc). Set EMSDK_DIR, defaults to /tmp/emsdk.
#   - a nightly toolchain: -Z build-std is required because the pthread build
#     recompiles compiler_builtins with atomics.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${EMSDK_DIR:=/tmp/emsdk}"
# emsdk_env.sh unsets EMSDK_DIR when sourced, so stash and restore it.
_SAVED_EMSDK_DIR="$EMSDK_DIR"
set +u
# shellcheck disable=SC1091
source "$EMSDK_DIR/emsdk_env.sh" >/dev/null 2>&1 || true
set -u
EMSDK_DIR="$_SAVED_EMSDK_DIR"
export PATH="$EMSDK_DIR/upstream/emscripten:$PATH"

command -v emcc >/dev/null || {
  echo "emcc not found. Install emsdk and set EMSDK_DIR (currently '$EMSDK_DIR')." >&2
  exit 1
}

# The browser transport runtime (SAB ring --js-library, --pre-js injection, and
# the Web Worker boot) is the SHARED @query-farm/vgi-worker-runtime package — the
# same files every browser VGI worker uses. We do NOT vendor per-worker copies: a
# stale ring twin fails in ways that look like data corruption. Source them from a
# checkout of that module (VGI_WORKER_RUNTIME), or fetch a pinned version from the
# CDN, e.g.:
#   V=0.1.0; b="https://cdn.jsdelivr.net/npm/@query-farm/vgi-worker-runtime@$V/wasm"
#   mkdir -p .vgi-runtime && for f in vgi_worker_lib.js vgi_worker_pre.js vgi-worker-boot.js; do
#     curl -sSfo ".vgi-runtime/$f" "$b/$f"; done
#   VGI_WORKER_RUNTIME=.vgi-runtime ./wasm/build.sh
: "${VGI_WORKER_RUNTIME:=../vgi-worker-runtime/wasm}"
: "${VGI_WORKER_LIB:=$VGI_WORKER_RUNTIME/vgi_worker_lib.js}"
: "${VGI_WORKER_PRE:=$VGI_WORKER_RUNTIME/vgi_worker_pre.js}"
[ -f "$VGI_WORKER_PRE" ] || {
  echo "Cannot find the VGI pthread-realm pre-js at: $VGI_WORKER_PRE" >&2
  echo "It ships in @query-farm/vgi-worker-runtime (wasm/vgi_worker_pre.js);" >&2
  echo "set VGI_WORKER_RUNTIME to a checkout/CDN copy, or VGI_WORKER_PRE directly." >&2
  exit 1
}
[ -f "$VGI_WORKER_LIB" ] || {
  cat >&2 <<MSG
Cannot find the VGI SAB ring js-library at:
  $VGI_WORKER_LIB

It ships in @query-farm/vgi-worker-runtime (wasm/vgi_worker_lib.js). Point
VGI_WORKER_RUNTIME at a checkout/CDN copy of that package's wasm/ dir, or set
VGI_WORKER_LIB directly.
MSG
  exit 1
}

TARGET=wasm32-unknown-emscripten
OUT=wasm/dist
mkdir -p "$OUT"

# +atomics,+bulk-memory are mandatory: -Z build-std recompiles compiler_builtins,
# and without them wasm-ld rejects --shared-memory. --no-entry is needed because
# a transitive dependency (crc-fast) declares a cdylib crate-type, which emcc
# would otherwise try to link as a program with a main().
# FIXEDFORMAT_BENCH=1 adds the OPFS load-test harness (see wasm/README.md).
CARGO_FEATURES=()
BENCH_EXPORTS=""
if [ -n "${FIXEDFORMAT_BENCH:-}" ]; then
  CARGO_FEATURES=(--features bench)
  : "${PTHREAD_POOL_SIZE:=12}"   # coordinator + scan threads exceed the 4-slot pool
  export PTHREAD_POOL_SIZE
  BENCH_EXPORTS=",_fixedformat_wasm_bench_opfs,_fixedformat_wasm_bench_parallel,_fixedformat_wasm_bench_parallel_s3"
  echo "==> bench harness ENABLED"
fi

echo "==> cargo build ($TARGET)"
RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals${EXTRA_TF:-} -C opt-level=${OPT_LEVEL:-3} -C link-args=-pthread -C link-arg=--no-entry" \
  cargo +nightly build \
  -p fixedformat-wasm "${CARGO_FEATURES[@]}" \
  --target "$TARGET" \
  -Z build-std=std,panic_abort \
  --release

LIB="target/$TARGET/release/libfixedformat_wasm.a"
[ -f "$LIB" ] || { echo "missing $LIB" >&2; exit 1; }

# PTHREAD_POOL_SIZE must be >= the channel slot count the host allocates (4), so
# every serve thread gets a pre-spawned pool worker.
#
# OPT_LEVEL=3 overrides the workspace's `opt-level = 2` release profile for this
# target only. It is worth ~2x on the parallel scan (663k -> 1.33M rows/s at 4
# threads) and does not change the native build. `+simd128` was measured
# separately and made NO difference, so it is deliberately not enabled — it would
# add a browser-compatibility constraint for nothing.
#
# MALLOC=mimalloc is load-bearing for multithreaded scans. Emscripten's default
# dlmalloc serializes every allocation on a global lock, and the decode path
# allocates per field per row — so concurrent serve threads convoy on it and
# aggregate throughput *falls* as threads are added (measured: 204k rows/s at 1
# thread down to 63k at 4). mimalloc is thread-caching: the same benchmark goes
# 397k → 508k → 641k, i.e. it scales about as well as native. Override with
# MALLOC=dlmalloc to compare.
#
# STACK_SIZE / DEFAULT_PTHREAD_STACK_SIZE: emscripten defaults to a 64 KiB stack,
# which this worker overflows — object_store + tokio + the Arrow encoders nest
# deeply, and an overflow surfaces as a bare "memory access out of bounds" with
# no console output, not a recognizable stack error. The serve threads are the
# ones that actually run scans, so the pthread stack matters most. INITIAL_MEMORY
# must cover main stack + every pthread stack up front, or startup stalls trying
# to grow shared memory while the pool is spawning.
echo "==> emcc link"
emcc wasm/main.c "$LIB" \
  --js-library wasm/vgi_http_lib.js \
  --js-library "$VGI_WORKER_LIB" \
  --pre-js "$VGI_WORKER_PRE" \
  -sMODULARIZE=1 -sEXPORT_NAME=VgiWorker \
  -pthread -sPTHREAD_POOL_SIZE=${PTHREAD_POOL_SIZE:-4} -sSHARED_MEMORY=1 \
  -fwasm-exceptions \
  -sENVIRONMENT=web,worker \
  -sEXPORTED_FUNCTIONS=_main,_vgi_worker_init,_vgi_worker_serve_sab_slot,_vgi_worker_serve_pool,_fixedformat_wasm_selftest_http,_fixedformat_wasm_mount_opfs,_fixedformat_wasm_opfs_selftest,_fixedformat_wasm_opfs_probe,_malloc,_free${BENCH_EXPORTS} \
  -sEXPORTED_RUNTIME_METHODS=HEAPU8,PThread,stringToNewUTF8 \
  -sEXIT_RUNTIME=0 -sALLOW_MEMORY_GROWTH=1 \
  -sWASMFS -sFORCE_FILESYSTEM \
  -sMALLOC=${MALLOC:-mimalloc} \
  -sSTACK_SIZE=1MB -sDEFAULT_PTHREAD_STACK_SIZE=2MB -sINITIAL_MEMORY=${INITIAL_MEMORY:-64MB} \
  -O${EMCC_OPT:-3} \
  -o "$OUT/vgi_worker.js"

# Stage the canonical boot alongside the module (referenced, not vendored — it
# is transport ABI shared with every VGI worker; see VGI_WORKER_BOOT).
: "${VGI_WORKER_BOOT:=$VGI_WORKER_RUNTIME/vgi-worker-boot.js}"
[ -f "$VGI_WORKER_BOOT" ] && cp "$VGI_WORKER_BOOT" "$OUT/vgi-worker-boot.js"

echo "built $OUT/vgi_worker.js + .wasm (+ vgi-worker-boot.js)"
ls -la "$OUT"
