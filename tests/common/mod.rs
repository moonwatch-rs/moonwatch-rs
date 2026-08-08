use std::path::PathBuf;

pub fn load_fixture(name: &str) -> String {
    std::fs::read_to_string(path_to_fixture(name)).unwrap()
}

pub fn path_to_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
