#!/usr/bin/env bash
# Build the fixedformat VGI worker and run the SQLLogic tests against it using
# the haybarn DuckDB distribution's unittest runner (which ships the `vgi`
# extension via the community repository).
#
# Prerequisites (one-time):
#   uv tool install --prerelease allow 'haybarn-unittest>=2.0.0rc11,<2.1'      # the DuckDB unittest binary
#   echo "INSTALL vgi FROM community;" | uvx --prerelease allow --from 'haybarn-cli>=2.0.0rc11,<2.1' haybarn-cli   # install the vgi ext
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

UNITTEST="${VGI_UNITTEST:-$(command -v haybarn-unittest || true)}"
if [[ -z "$UNITTEST" || ! -x "$UNITTEST" ]]; then
    echo "ERROR: haybarn-unittest not found. Install it with:" >&2
    echo "       uv tool install --prerelease allow 'haybarn-unittest>=2.0.0rc11,<2.1'" >&2
    exit 1
fi

# Ensure the vgi community extension is installed for this haybarn version.
if ! echo "LOAD vgi;" | uvx --prerelease allow --from 'haybarn-cli>=2.0.0rc11,<2.1' haybarn-cli >/dev/null 2>&1; then
    echo "==> Installing vgi extension from community repository"
    echo "INSTALL vgi FROM community;" | uvx --prerelease allow --from 'haybarn-cli>=2.0.0rc11,<2.1' haybarn-cli
fi

echo "==> Building fixedformat-worker (release)"
cargo build --release --bin fixedformat-worker

# NOTE: this is a Catch2 test-name filter, not a shell glob. Catch2 only honors
# a trailing `*` wildcard, so use `test/sql/*` (not `test/sql/*.test`).
WORKER="$REPO_ROOT/target/release/fixedformat-worker"
TEST_GLOB="${1:-test/sql/*}"

echo "==> Running SQLLogic tests"
echo "    worker:   $WORKER"
echo "    unittest: $UNITTEST"
echo "    tests:    $TEST_GLOB"

VGI_TEST_WORKER="$WORKER" \
VGI_WORKER_CATALOG_NAME="fixed" \
    "$UNITTEST" --keep-home --test-dir "$REPO_ROOT" "$TEST_GLOB"
