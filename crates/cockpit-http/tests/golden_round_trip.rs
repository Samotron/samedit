//! Round-trip golden tests for the `.bru` fixtures under
//! `tests/fixtures/http/`.
//!
//! These run the full parse → serialise loop on every fixture and assert
//! that the second parse yields the exact same model. The serialiser is
//! opinionated about layout (canonical block order, 2-space indent inside
//! body blocks), so byte-identical round-tripping is *not* the contract
//! here — semantic round-tripping is.

use std::fs;
use std::path::PathBuf;

use cockpit_http::{parse_request, serialise_request};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("http")
}

fn fixture_files() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(fixtures_dir())
        .expect("fixtures/http exists")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("bru"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_fixture_parses() {
    let paths = fixture_files();
    assert!(!paths.is_empty(), "expected at least one .bru fixture");
    for path in paths {
        let source = fs::read_to_string(&path).expect("read fixture");
        if let Err(err) = parse_request(&source) {
            panic!("{}: {err}", path.display());
        }
    }
}

#[test]
fn every_fixture_round_trips_through_parse_and_serialise() {
    for path in fixture_files() {
        let source = fs::read_to_string(&path).expect("read fixture");
        let first = parse_request(&source)
            .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));
        let rendered = serialise_request(&first);
        let second = parse_request(&rendered).unwrap_or_else(|err| {
            panic!(
                "{}: serialised form failed to reparse: {err}\n--- rendered ---\n{rendered}",
                path.display()
            )
        });
        assert_eq!(first, second, "{}: round-trip diverged", path.display(),);
    }
}
