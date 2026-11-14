# Fixed-format notebooks

[A perfect round trip can preserve the wrong amount](01_a_round_trip_does_not_validate_the_scale.ipynb)
uses a six-record synthetic ledger to distinguish byte preservation from correct
decimal interpretation. It demonstrates `describe_fixed`, `unpack_fixed`, `pack_fixed`
and `read_fixed` through visible SQL, with a field-boundary diagram and an account
reconciliation chart. Successful outputs are embedded in the committed notebook.

## Run

Install `uv`, then run from this directory:

```sh
uv sync --frozen --python 3.14
uv run --frozen python execute.py
uv run --frozen python execute.py --parameters '{"AMOUNT_SCALE": 3}' --output-dir validation/scale-3
```

The literal connection is `github-auto://Query-farm/vgi-fixedformat@v0.12.0`.
Haybarn 2.0.0rc11 retrieves the native worker release for the reader's platform.
Initial dependency/extension/release installation needs network access; the analysis
then uses local synthetic inputs. There is no API credential, Rust build, engine build
or dependency on the worker checkout. Copy the complete `notebooks/` directory to run
it independently; keep its assets, helper and locked environment together.

For interactive use, choose `notebooks/.venv/bin/python` in your notebook editor,
or register that environment:

```sh
uv run --frozen python -m ipykernel install --user --name vgi-fixedformat-notebooks --display-name "Python (vgi-fixedformat notebooks)"
```

`execute.py` creates a temporary kernel specification, runs with this directory as cwd,
and saves atomically after successful execution. Overrides require a separate output
directory. A failed run preserves the successful default. Experiment copies include
branding, documentation, helper code and the environment lock. Restart and Run All
after changing the visible `AMOUNT_SCALE` parameter.

## Data and interpretation

The raw ASCII strings and independent synthetic control manifest are visible SQL
literals. Each 24-byte record has a six-byte account, one-byte direction, eight-byte
reference and nine display digits for the amount. The fixture's documented scale is
two decimal places; `D` adds and `C` subtracts under this example's convention.
These are invented illustrative USD values, not real financial records or a universal
accounting sign convention.

The default totals are 154.25 and 87.59, matching the fixed control manifest's combined
241.84. Setting `AMOUNT_SCALE=3` yields 15.425 and 8.759, totaling 24.184. Both scales
re-create every original byte. The wrong scale is intentionally allowed as an
experiment: the independent control status changes from two reconciled accounts to
zero, while the structural checks remain valid. Those checks must not be confused
with semantic acceptance. Even matching totals can hide offsetting record errors.

The file example writes the same raw records to an owned temporary file without line
breaks, then reads them with fixed framing, record length 24, ASCII encoding and no
compression. It compares scalar and file results including duplicate multiplicity.
The temporary file is removed during cleanup. Other encodings, framing modes, cloud
storage and the browser/WASM build are outside this notebook's scope.

Nine SQL checks validate layout coverage, record length, identifiers, direction values,
exact re-encoding, implied scale, file/scalar agreement, control coverage and exact
reconciliation classification. The amounts and control comparisons remain DECIMAL
in SQL; plotting converts the displayed values to floating point only for rendering.

## Validation and upgrading

The default and a separate scale-three experiment are executed in fresh kernels;
the experiment also runs from a detached copy of the complete notebook directory.
All four rendered charts are inspected, and three runner guards verify that rejected
overrides or a failing run do not replace the saved default.

The unchanged worker source passes 176 Rust unit/property tests across the workspace
with all features, Clippy with warnings denied, formatting and its SQLLogic suite:
458 assertions in 20 cases; one S3 test is skipped because no test bucket is configured.
Executed source metadata lint reports 100/100, L2, zero findings. The notebook uses
the published native release, independently of these source-validation builds.

To upgrade the worker, edit the visible release tag and rerun the default, experiment
and checks. To upgrade the notebook environment, update `pyproject.toml`, regenerate
only this directory's `uv.lock`, and repeat validation. Root manifests and locks are
preserved.

The final default completed in 1.8 seconds and the detached scale-three experiment
in 13.4 seconds. Cargo audit passed with the repository’s existing
`RUSTSEC-2025-0141` unmaintained-dependency exception; no new exception was added.
