#!/bin/sh
# Copyright 2026 Query Farm LLC - https://query.farm
#
# Dispatch the single vgi-fixedformat image into one of its transports:
#   http   (default) the HTTP server on $PORT (8000), bound 0.0.0.0 so a
#                    published host port reaches it. Serves /health.
#   tcp              raw Arrow-IPC over TCP on $PORT_TCP (8001), bound 0.0.0.0.
#                    Used by the VGI extension's transparently-shared container.
#   stdio            a worker DuckDB spawns over stdio (on-host execution).
# Any other first argument is exec'd verbatim (escape hatch for debugging).
#
# The worker is stateless (files are named at query time), so there is no /data
# to create and no state env to wire — each mode just exec's the binary.
set -e

case "${1:-http}" in
  http)
    shift 2>/dev/null || true
    # `--http` reads its bind address from VGI_HTTP_BIND (default 127.0.0.1:0,
    # an ephemeral loopback port). In a container we must bind 0.0.0.0 on a
    # FIXED port so `-p $PORT:$PORT` and the HEALTHCHECK reach it.
    export VGI_HTTP_BIND="0.0.0.0:${PORT:-8000}"
    exec fixedformat-worker --http "$@"
    ;;
  tcp)
    shift 2>/dev/null || true
    exec fixedformat-worker --tcp "0.0.0.0:${PORT_TCP:-8001}" "$@"
    ;;
  stdio)
    shift 2>/dev/null || true
    exec fixedformat-worker "$@"
    ;;
  *)
    exec "$@"
    ;;
esac
