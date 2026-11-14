//! The files checked in under oss/generated/ are what `zirc_core::generated` makes now.

use std::path::{Path, PathBuf};

fn generated_dir() -> PathBuf {
    // Under Bazel, beside schema.json in the runfiles; under Cargo, in the source tree.
    std::env::var_os("ZIRC_GENERATED_SCHEMA").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../generated"),
        |schema| Path::new(&schema).parent().unwrap().to_owned(),
    )
}

#[test]
fn the_generated_files_are_made_from_the_rust_types() {
    let dir = generated_dir();
    for (path, made) in zirc_core::generated::files() {
        let checked_in = std::fs::read_to_string(dir.join(path)).unwrap_or_default();
        assert!(
            checked_in == made,
            "oss/generated/{path} isn't what the Rust types make: run bazel run //crates/zirc-core:generated_types"
        );
    }
}
