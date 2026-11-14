//! Composed example queries — each combines two or more of the worker's own
//! objects in one realistic query (a scan feeding a writer, a multi-record
//! read reconciled against its own trailer).
//!
//! Each SQL is defined ONCE here and spliced into both the schema-level
//! `vgi.example_queries` list (`lib.rs`) and the example list of the object
//! that drives the query (`buffering/write_fixed.rs`, `table/read_multi.rs`),
//! within that object's first five entries so the vgi-lint agent simulator
//! (which only shows per-object examples) sees them.

/// One `{description, sql}` entry of a `vgi.example_queries` list.
pub struct Example {
    pub description: &'static str,
    pub sql: &'static str,
}

/// Mainframe hand-off: scan an ASCII newline feed with `read_fixed` and stream
/// it straight into `write_fixed` as fixed-length EBCDIC records with a COMP-3
/// quantity (`data/accounts.dat` has 3 records → 3 × 13 bytes).
pub const ASCII_TO_EBCDIC_COMP3: Example = Example {
    description: "Convert an ASCII newline feed (10-char name, 5-digit qty) into a fixed-length EBCDIC mainframe file with a packed-decimal COMP-3 quantity by piping read_fixed into write_fixed; returns 3 rows and 39 bytes (13-byte records, no separators).",
    sql: "SELECT rows_written, bytes_written FROM fixed.main.write_fixed((FROM fixed.main.read_fixed('data/accounts.dat', 'name:A10 qty:9(5)')), 'data/_example_ebcdic.dat', '01 ACCT. 05 NAME PIC X(10). 05 QTY PIC S9(5) COMP-3.', encoding => 'ebcdic', framing => 'fixed')",
};

/// Split the detail records out of a header/detail/trailer file into a flat
/// single-layout file: `read_multi` → `union_extract` → `write_fixed`.
pub const SPLIT_DETAILS_TO_FLAT: Example = Example {
    description: "Extract only the detail (D) records of a header/detail/trailer file with read_multi and write them to a flat sku/qty fixed-width file with write_fixed, dropping the header and trailer; returns 2 rows and 32 bytes.",
    sql: "SELECT rows_written, bytes_written FROM fixed.main.write_fixed((SELECT d.sku, d.qty FROM (SELECT union_extract(record, 'D') AS d FROM fixed.main.read_multi('data/multi.dat', '{\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"co\",\"type\":\"str\",\"width\":20}],\"D\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"sku\",\"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\"digits\":5}],\"T\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"cnt\",\"type\":\"int\",\"digits\":6}]}}') WHERE union_tag(record) = 'D')), 'data/_example_details.dat', 'sku:A10 qty:9(5)')",
};

/// Batch-control check: count the detail records and compare against the
/// trailer's declared record count, alongside the header's company name.
pub const RECONCILE_TRAILER: Example = Example {
    description: "Reconcile a header/detail/trailer batch: report the header's company, the number of detail records and their total qty, the trailer's declared record count, and whether the detail count matches it (ACME CORP, 2 details, qty 49, trailer 2, balanced).",
    sql: "WITH r AS (SELECT record FROM fixed.main.read_multi('data/multi.dat', '{\"discriminator\":{\"offset\":0,\"width\":1},\"records\":{\"H\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"co\",\"type\":\"str\",\"width\":20}],\"D\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"sku\",\"type\":\"str\",\"width\":10},{\"name\":\"qty\",\"type\":\"int\",\"digits\":5}],\"T\":[{\"type\":\"filler\",\"width\":1},{\"name\":\"cnt\",\"type\":\"int\",\"digits\":6}]}}')) SELECT max(union_extract(record, 'H').co) AS company, count(*) FILTER (WHERE union_tag(record) = 'D') AS detail_records, sum(union_extract(record, 'D').qty) AS total_qty, max(union_extract(record, 'T').cnt) AS trailer_count, count(*) FILTER (WHERE union_tag(record) = 'D') = max(union_extract(record, 'T').cnt) AS balanced FROM r",
};

/// Every composed example, in the order the schema-level list shows them.
pub const COMPOSED: &[&Example] = &[
    &ASCII_TO_EBCDIC_COMP3,
    &SPLIT_DETAILS_TO_FLAT,
    &RECONCILE_TRAILER,
];

/// JSON-escape a string for embedding in a hand-built JSON document.
pub fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Append `extra` entries to a hand-written `vgi.example_queries` JSON list
/// (`base` must be a JSON array literal ending in `]`).
pub fn with_examples(base: &str, extra: &[&Example]) -> String {
    let body = base
        .trim_end()
        .strip_suffix(']')
        .expect("example list must be a JSON array")
        .trim_end();
    let mut out = body.to_string();
    let mut empty = body.trim_start().strip_prefix('[').map(str::trim) == Some("");
    for ex in extra {
        if !empty {
            out.push(',');
        }
        empty = false;
        out.push_str(&format!(
            "\n  {{\n    \"description\": \"{}\",\n    \"sql\": \"{}\"\n  }}",
            json_escape(ex.description),
            json_escape(ex.sql)
        ));
    }
    out.push_str("\n]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_valid_json_entries() {
        let base = "[\n  {\"description\": \"a\", \"sql\": \"SELECT 1\"}\n]";
        let out = with_examples(base, COMPOSED);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1 + COMPOSED.len());
        for (entry, ex) in arr[1..].iter().zip(COMPOSED) {
            assert_eq!(entry["description"], ex.description);
            assert_eq!(entry["sql"], ex.sql);
        }
    }

    #[test]
    fn appends_to_empty_list() {
        let out = with_examples("[]", &[&RECONCILE_TRAILER]);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[test]
    fn composed_examples_are_catalog_qualified_one_line() {
        for ex in COMPOSED {
            assert!(!ex.sql.contains('\n'), "{}", ex.sql);
            assert!(ex.sql.contains("fixed.main."), "{}", ex.sql);
            // Every worker-object reference is catalog-qualified (VGI505).
            for obj in ["read_fixed(", "read_multi(", "write_fixed(", "pack_fixed("] {
                assert_eq!(
                    ex.sql.matches(obj).count(),
                    ex.sql.matches(&format!("fixed.main.{obj}")).count(),
                    "{}",
                    ex.sql
                );
            }
        }
    }
}
