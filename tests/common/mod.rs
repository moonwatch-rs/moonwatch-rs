use std::path::PathBuf;

// Each integration test binary compiles this module separately, so a helper only some of
// them need would otherwise warn as dead code.
#[allow(dead_code)]
pub fn load_fixture(name: &str) -> String {
    std::fs::read_to_string(path_to_fixture(name)).unwrap()
}

pub fn path_to_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
