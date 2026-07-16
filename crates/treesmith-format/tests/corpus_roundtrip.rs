//! Corpus walker (DESIGN.md §14): every sniffable `.yml` under `fixtures/`
//! must round-trip byte-identically (spec I2, CI gate zero).
//!
//! Walks `fixtures/` from `CARGO_MANIFEST_DIR/../..`; skips silently if the
//! directory is absent so the Format phase can land before the Fixtures
//! phase.

use std::path::Path;

#[test]
fn every_sniffable_fixture_round_trips_byte_identically() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let fixtures = root.join("fixtures");
    if !fixtures.is_dir() {
        return; // fixtures phase not landed yet
    }

    let fmt = treesmith_format::detect(&fixtures);
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for entry in walkdir::WalkDir::new(&fixtures)
        .sort_by_file_name()
        .into_iter()
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str() else {
            continue;
        };
        if !fmt.sniff_file_name(name) {
            continue;
        }
        let bytes = std::fs::read(entry.path()).expect("read fixture");
        let head_len = bytes.len().min(512);
        if !fmt.sniff_head(&bytes[..head_len]) {
            continue;
        }
        checked += 1;
        match fmt.parse(&bytes) {
            Err(f) => failures.push(format!("{}: parse fault {f}", entry.path().display())),
            Ok(item) => {
                let emitted = fmt.emit(&item);
                if emitted != bytes {
                    failures.push(format!(
                        "{}: emit differs from source ({} vs {} bytes)",
                        entry.path().display(),
                        emitted.len(),
                        bytes.len()
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "round-trip failures:\n{}",
        failures.join("\n")
    );
    assert!(
        checked > 0,
        "fixtures/ exists but contained no sniffable item files"
    );

    // The census harness must agree: zero faults, zero mismatches.
    let census = treesmith_format::census::round_trip_census(&fixtures, fmt);
    assert_eq!(census.items, checked);
    assert_eq!(census.ok, checked);
    assert!(census.faults.is_empty(), "faults: {:?}", census.faults);
    assert!(
        census.mismatches.is_empty(),
        "mismatches: {:?}",
        census.mismatches
    );
}
