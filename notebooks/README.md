# Fixed-width records through SQL

| Notebook | Question and experiment |
| --- | --- |
| [01 · A perfect round trip can preserve the wrong amount](01_a_round_trip_does_not_validate_the_scale.ipynb) | Decode six synthetic ledger records, inspect byte boundaries, repack them and reconcile independent account controls. Move the implied scale from 2 to 3 without changing any stored byte. |
| [02 · Filter details, rebuild the envelope](02_filter_details_rebuild_the_envelope.ipynb) | Validate a synthetic header/detail/trailer file, select detail lines, rebuild its trailer and write mixed and flat local exports. Raise the minimum quantity from 10 to 20. |

Use the **Open in Colab** button in either notebook, run its install cell, then **Run all**.
Each example is independent: all fixture bytes are visible in SQL and no local worker
checkout, companion file, private data or account is required. The base shared package
preserves compatible preloaded plotting/data libraries; the optional local runner is
separate. Fresh Python 3.12 execution with preloaded libraries is tested. An actual
hosted Colab session has not been tested.

For local publication or execution, install Python 3.11+ and [uv](https://docs.astral.sh/uv/):

```sh
cd notebooks
uv sync --locked --extra runner
uv run --locked --extra runner vgi-notebooks
```

To register an interactive kernel, use
`uv run --locked --extra runner python -m ipykernel install --user --name vgi-fixedformat-notebooks --display-name 'VGI Fixedformat notebooks'`.
Select it in Jupyter, restart and run all after changing parameters.

Both examples show the real connection SQL:

```sql
INSTALL vgi FROM community;
LOAD vgi;
ATTACH 'fixed' (TYPE vgi,
 LOCATION 'github-auto://Query-farm/vgi-fixedformat@v0.12.0');
```

Haybarn 2.0.0rc11 retrieves the platform release, verifies its published asset and
starts the worker. No Rust compiler or dependency overrides are needed. First use
requires network access for the shared Python package, extension and native worker;
subsequent calculations and file operations are local. Native validation ran on
macOS Apple silicon. Release assets determine support for other platforms.

Each shared `connect(globals())` call opens an in-memory database with an empty
temporary SQL-secret directory and persistent secrets disabled. Both notebooks
create only their own temporary files, close the connection/worker and delete those
files at the end. The second notebook's write calls are materialized once before
capture/display. No S3, HTTP, real account or external filesystem destination is used.

## Independent parameter experiments

Run each changed parameter in its own output directory:

```sh
uv run --locked --extra runner vgi-notebooks \
  01_a_round_trip_does_not_validate_the_scale.ipynb \
  --parameters '{"AMOUNT_SCALE":3}' --output-dir validation/scale-3

uv run --locked --extra runner vgi-notebooks \
  02_filter_details_rebuild_the_envelope.ipynb \
  --parameters '{"MIN_QTY":20}' --output-dir validation/quantity-20
```

The ledger experiment keeps all six 24-byte records identical after repacking, while
both accounts stop reconciling: decoded net total changes from 241.84 to 24.184
synthetic USD, and the independent controls remain 241.84. A perfect round trip does
not establish a correct schema interpretation.

The shipment experiment retains two lines / 65 units instead of three / 77. The
corrected mixed export shrinks from 61 to 48 bytes and the flat extract from 36 to
24 bytes, including newlines. The original file stays at four details / 84 units
and 74 bytes. A flat extract intentionally omits the batch header and trailer; its
smaller size is a different data contract, not a compression result.

`AMOUNT_SCALE` accepts integer 2 or 3. `MIN_QTY` accepts integers 0–40 so at least one
fixture detail remains selected. A zero-detail delivery needs an explicit consumer
contract and is outside this example. Boolean parameters are rejected. Invalid
overrides and failed runs preserve the last successful default and experiment.

## Provenance and interpretation

These are authored synthetic teaching records, unrelated to customer or production
data. In the ledger, `D` adds and `C` subtracts only under the stated fixture convention;
this is not a general accounting rule. Independent controls test aggregate agreement,
which cannot detect every offsetting row error. Values remain SQL DECIMALs through
all parsing and reconciliation.

The shipment workflow inspects layouts and vocabulary, validates incoming controls
before selecting output, preserves tagged-union NULL semantics, and checks the
actual written files. Header/trailer consistency does not prove that a source shipment
is true. Examples use uncompressed ASCII; EBCDIC, remote storage, binary framing,
compression and arbitrary copybook behavior are not inferred from these results.

## Shared presentation and coverage

The [shared standard and runner](https://github.com/Query-farm/vgi-example-notebooks/blob/v0.3.6/docs/standard.md)
are pinned at **v0.3.6**. Presentation and execution helpers are not copied here.
Default figures retain native SVG and content-addressed `charts/` assets for GitHub;
experiment copies use native SVG. Preserve older chart assets when updating examples
so earlier saved notebook revisions continue to render.

[Coverage evidence](coverage.json) verifies **10 of 12** named public SQL surfaces
for both released v0.12.0 and the reviewed current-public interface. The denominator
comes from complete actual function/table/view catalogs, including overloads grouped
by `(schema,name)`. The two COPY handlers `copy_fixed` and `copy_to_fixed` remain in
the denominator and are explicitly uncovered; their COPY-statement syntax is not
presented as an executed table-function example. For a catalog larger than ten,
the required threshold is six. The native `spec_reference` view is included and
interpreted when choosing framing and encoding. Current-source catalog discovery
is separate from notebook execution against the native release; reviewed source
differences add metadata/composed examples without changing domain logic.

`coverage/` contains the raw catalog exports, reviewed snapshots, post-execution
receipts and validation result. Notebook 01 keeps its original nine SQL consistency
checks. Notebook 02 adds twelve SQL invariants, a pre-export acceptance assertion
and two comparisons with actual file byte sizes.
