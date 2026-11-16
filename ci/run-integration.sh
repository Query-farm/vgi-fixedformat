#!/usr/bin/env bash
# Adapted from the vgi-starters run-integration.sh (rust).
#
# Run test/sql/*.test through DuckDB (the haybarn-unittest sqllogictest runner)
# against the vgi-fixedformat worker, over one transport.
#
#   WORKER=/abs/path/to/fixedformat-worker TRANSPORT=stdio ci/run-integration.sh
#
# Optional environment:
#   WORKER            absolute path to the worker executable: no arguments =
#                     stdio; `--http --port 0` = HTTP, printing `PORT:<n>` on
#                     stdout once listening. (default: target/release/fixedformat-worker)
#   TRANSPORT         stdio | launch | http                        (default: stdio)
#   HAYBARN_UNITTEST  path to haybarn-unittest   (default: the one on PATH)
#   MIN_EXECUTED      fail if fewer test cases ran (default: 1)
#   EXPECTED_SKIP_REASONS  newline-separated skip reasons that are allowed,
#                     e.g. "require-env SOME_LIVE_VAR". Any other skip fails.
#   TEST_PATTERN      runner pattern under the staged tree (default: test/sql/*)
#   STAGE             scratch dir for the staged test tree (default: a temp dir,
#                     removed on exit; a caller-supplied STAGE is kept)
#   VGI_TEST_WORKER  preset LOCATION for the Docker image test: a stdio
#                     command (`docker run -i … stdio`) with TRANSPORT=stdio, or
#                     an http(s):// URL with TRANSPORT=http (no local worker).
#   WARMUP_TIMEOUT    seconds allowed for the extension install (default: 300)
#   KEEP_STAGE=1      keep the staged test tree for debugging
#
# Transports:
#   stdio   DuckDB spawns the worker and talks over stdin/stdout (the default).
#   launch  DuckDB starts the worker once on an AF_UNIX socket and pools
#           connections (`LOCATION 'launch:/path'`); best for slow-starting
#           runtimes such as the JVM.
#   http    the worker runs as an HTTP server (`--http`), as in Docker deploys.
#
# Why the extra checks: the sqllogictest runner turns some errors into silent
# SKIPs (anything mentioning "HTTP" without --test-config) and exits 0 when no
# test matched. Both would make a broken lane look green, so this script passes
# a config that disables error-skipping and fails on any unexpected skip.
set -euo pipefail

TRANSPORT="${TRANSPORT:-stdio}"
HAYBARN_UNITTEST="${HAYBARN_UNITTEST:-$(command -v haybarn-unittest || true)}"
MIN_EXECUTED="${MIN_EXECUTED:-1}"
[ -n "$HAYBARN_UNITTEST" ] || { echo "::error::haybarn-unittest not found (make tools)"; exit 1; }

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORKER="${WORKER:-$REPO/target/release/fixedformat-worker}"
OWN_STAGE=0
if [ -z "${STAGE:-}" ]; then STAGE="$(mktemp -d)"; OWN_STAGE=1; fi
mkdir -p "$STAGE"
STAGE="$(cd "$STAGE" && pwd)"
PIDS=()
HTTP_PID=""
cleanup() {
  for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null || true; done
  if [ "${KEEP_STAGE:-0}" = "1" ]; then echo "stage kept at $STAGE"
  elif [ "$OWN_STAGE" = "1" ]; then rm -rf "$STAGE"; fi
}
trap cleanup EXIT

# start_server <log> <cmd...> — run a server that prints PORT:<n>; sets PORT.
# Servers run from $STAGE, the runner's working directory, so relative paths in
# the tests resolve the same way in DuckDB and in the worker.
PORT=""
start_server() {
  local log="$1"; shift
  # Create the log before backgrounding: the child opens its redirect only
  # once it is scheduled, and the poll below would otherwise read a missing
  # file, which under set -e -o pipefail kills the whole run.
  : > "$log"
  (cd "$STAGE" && exec "$@") >"$log" 2>&1 &
  local pid=$!
  PIDS+=("$pid")
  for _ in $(seq 1 120); do
    kill -0 "$pid" 2>/dev/null || { echo "::error::'$*' exited early:"; cat "$log"; return 1; }
    PORT="$(sed -n 's/^PORT:\([0-9][0-9]*\).*/\1/p' "$log" | head -1)"
    [ -n "$PORT" ] && return 0
    sleep 0.5
  done
  echo "::error::'$*' never printed PORT:<n>"; cat "$log"; return 1
}

export VGI_WORKER_CATALOG_NAME=fixed
# test/sql/s3_roundtrip.test needs an S3-compatible server (CI's cloud-e2e job
# starts moto and sets these); without one it is an expected skip.
if [ -z "${FIXEDFORMAT_S3_TEST_BUCKET:-}" ]; then
  EXPECTED_SKIP_REASONS="${EXPECTED_SKIP_REASONS:-}"$'\n'"require-env FIXEDFORMAT_S3_TEST_BUCKET"
fi

HTTP=0
case "$TRANSPORT" in
  stdio)  export VGI_TEST_WORKER="${VGI_TEST_WORKER:-$WORKER}" ;;
  launch) export VGI_TEST_WORKER="launch:$WORKER" ;;
  http)
    HTTP=1
    if [[ "${VGI_TEST_WORKER:-}" =~ ^https?:// ]]; then
      echo "Using pre-launched HTTP worker at $VGI_TEST_WORKER"
    else
      start_server "$STAGE/worker.log" "$WORKER" --http --port 0
      HTTP_PID="${PIDS[${#PIDS[@]}-1]}"
      export VGI_TEST_WORKER="http://127.0.0.1:$PORT"
    fi
    ;;
  *) echo "::error::unknown TRANSPORT=$TRANSPORT (expected stdio|launch|http)"; exit 1 ;;
esac

# Stage the tests, rewriting `require vgi` / `require <core ext>` into a LOAD of
# the signed extension installed below (the standalone runner links no
# extensions), and loading httpfs for HTTP workers (after a test's own
# `LOAD vgi;`, or before its first ATTACH when it relies on autoloading vgi).
mkdir -p "$STAGE/test/sql"
for f in "$REPO"/test/sql/*.test; do
  awk -v http="$HTTP" '
    # Hold a `statement ok` until the next line shows whether it ATTACHes the
    # worker: tests that rely on autoloading vgi need httpfs loaded first.
    held { held = 0
      if (http == 1 && !hf && $0 ~ /ATTACH .*TYPE vgi/) {
        print "statement ok"; print "LOAD httpfs;"; print ""; hf = 1
      }
      print "statement ok" }
    /^statement ok[ \t]*$/ { held = 1; next }
    /^require[ \t]+vgi[ \t]*$/ {
      print "statement ok"; print "LOAD vgi;"; print ""
      if (http == 1) { print "statement ok"; print "LOAD httpfs;"; print ""; hf = 1 }
      next
    }
    /^LOAD[ \t]+vgi[ \t]*;[ \t]*$/ {
      print
      if (http == 1) { print ""; print "statement ok"; print "LOAD httpfs;"; hf = 1 }
      next
    }
    /^require[ \t]+(httpfs|json|parquet|inet)[ \t]*$/ {
      print "statement ok"; print "LOAD " $2 ";"; next
    }
    { print }' "$f" > "$STAGE/test/sql/$(basename "$f")"
done
# The tests read and write 'data/...' fixtures relative to $STAGE, so the
# round-trip outputs land in the stage, not the checkout.
mkdir -p "$STAGE/data"
cp -R "$REPO/data/." "$STAGE/data/"

# Install the extensions once, refreshing any stale cached copy of vgi.
cat > "$STAGE/test/_warm.test" <<'EOF'
statement ok
FORCE INSTALL vgi FROM community;

statement ok
INSTALL httpfs FROM core;

statement ok
INSTALL json FROM core;
EOF
# Bounded: a stalled extension download would otherwise hang the lane until the
# job timeout. WARMUP_TIMEOUT is in seconds.
( cd "$STAGE" && exec "$HAYBARN_UNITTEST" --keep-home --test-dir "$STAGE" "test/_warm.test" ) >/dev/null 2>&1 &
warm_pid=$!
warm_deadline=$((SECONDS + ${WARMUP_TIMEOUT:-300}))
while kill -0 "$warm_pid" 2>/dev/null && [ "$SECONDS" -lt "$warm_deadline" ]; do sleep 1; done
if kill -0 "$warm_pid" 2>/dev/null; then
  kill "$warm_pid" 2>/dev/null || true
  wait "$warm_pid" 2>/dev/null || true
  echo "::warning::extension warm-up timed out after ${WARMUP_TIMEOUT:-300}s (stalled download?)"
elif ! wait "$warm_pid"; then
  echo "::warning::extension warm-up did not fully succeed"
fi
rm -f "$STAGE/test/_warm.test"

cat > "$STAGE/no_error_skip.json" <<'EOF'
{"skip_error_messages": ["__vgi_starter_never_matches__"]}
EOF

TEST_PATTERN="${TEST_PATTERN:-test/sql/*}"
echo "Running $TEST_PATTERN over $TRANSPORT (worker: $VGI_TEST_WORKER)"
LOG="$STAGE/unittest.log"
rc=0
( cd "$STAGE" && "$HAYBARN_UNITTEST" --keep-home --test-dir "$STAGE" \
    --test-config "$STAGE/no_error_skip.json" "$TEST_PATTERN" ) 2>&1 | tee "$LOG" && rc=0 || rc="${PIPESTATUS[0]}"

if grep -q 'No test cases matched\|No tests ran' "$LOG"; then
  echo "::error::no test cases matched; the staging is wrong"; exit 1
fi
total="$(sed -n 's/^\[[0-9]*\/\([0-9]*\)\].*/\1/p' "$LOG" | tail -1)"
total="${total:-0}"
skips="$(awk '/^Skipped tests for the following reasons:/{b=1;next} b&&/^[[:space:]]*$/{b=0} b&&match($0,/: [0-9]+[[:space:]]*$/){print substr($0,1,RSTART-1) "\t" substr($0,RSTART+2)+0}' "$LOG")"
skipped=0
while IFS=$'\t' read -r reason n; do
  [ -n "$reason" ] || continue
  skipped=$((skipped + n))
  if ! printf '%s\n' "${EXPECTED_SKIP_REASONS:-}" | grep -qxF "$reason"; then
    echo "::error::unexpected skip: '$reason'. A test file silently stopped running;" \
         "fix the gate or add it to EXPECTED_SKIP_REASONS."
    rc=1
  fi
done <<< "$skips"
executed=$((total - skipped))
echo "executed $executed of $total test files over $TRANSPORT ($skipped skipped)"
if [ "$executed" -lt "$MIN_EXECUTED" ]; then
  echo "::error::only $executed test files ran (MIN_EXECUTED=$MIN_EXECUTED)"; rc=1
fi
if [ -n "$HTTP_PID" ] && ! kill -0 "$HTTP_PID" 2>/dev/null; then
  echo "::error::the HTTP worker died during the run:"; cat "$STAGE/worker.log"; rc=1
fi
exit "$rc"
