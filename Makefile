# vgi-fixedformat worker — common tasks. `make help` lists them.
# Requires Rust (rustup) and uv (https://docs.astral.sh/uv/).

HAYBARN_VERSION  ?= 2.0.1rc3
BIN              := fixedformat-worker
WORKER           := $(CURDIR)/target/release/$(BIN)
TOOLS            := $(CURDIR)/.tools
HAYBARN_UNITTEST := $(TOOLS)/bin/haybarn-unittest
# The SQL suite runs over each of these (ci/run-integration.sh).
TRANSPORTS       ?= stdio launch http

.DEFAULT_GOAL := help
.PHONY: help build tools fmt lint test-unit test-sql test lint-explain vgi-lint check clean

help: ## List the targets
	@awk 'BEGIN{FS=":.*## "} /^[a-zA-Z_-]+:.*## /{printf "  %-13s %s\n",$$1,$$2}' $(MAKEFILE_LIST)

build: ## Build the worker (release)
	cargo build --locked --release --bin $(BIN)

tools: ## Install the pinned SQL test runner into .tools
	@test -x $(HAYBARN_UNITTEST) || { uv venv -q $(TOOLS) && uv pip install -q -p $(TOOLS) haybarn-unittest==$(HAYBARN_VERSION); }

fmt: ## Format the code
	cargo fmt --all

lint: ## Static checks: rustfmt, clippy
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings

test-unit: ## Unit tests (cargo test)
	cargo test --workspace --all-features

test-sql: build tools ## SQL tests through DuckDB over stdio, launch and http (TRANSPORTS="stdio")
	@set -e; for t in $(TRANSPORTS); do \
	  echo "== test/sql over $$t"; \
	  WORKER="$(WORKER)" HAYBARN_UNITTEST="$(HAYBARN_UNITTEST)" TRANSPORT=$$t ci/run-integration.sh; \
	done

test: test-unit test-sql ## Unit + SQL tests

lint-explain: build ## Metadata quality gate (vgi-lint at fail-on=info, as in CI)
	uvx --prerelease=allow --from 'vgi-lint-check>=2,<3' vgi-lint lint "$(WORKER)" --fail-on info

vgi-lint: lint-explain ## Alias for lint-explain

check: lint test-unit test-sql lint-explain ## Everything CI runs. Done = this passes.

clean: ## Remove build outputs and tool caches
	cargo clean
	rm -rf .tools
