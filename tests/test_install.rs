use std::fs;

use tempfile::tempdir;

use moonwatch_rs::installer::{install_files, EXECUTABLE_NAME};

/// Everything `moonwatch_rs install` puts into the installation directory. The autostart
/// entry and the running daemon are deliberately left out - those are what `install_files`
/// exists to be separable from.
const INSTALLED_FILES: [&str; 7] = [
    EXECUTABLE_NAME,
    "main_config.json",
    "recorder_config.json",
    "pipeline_config.json",
    "schemas/main_config.schema.json",
    "schemas/recorder_config.schema.json",
    "schemas/pipeline_config.schema.json",
];

/// Stands in for the downloaded binary. Its contents are never executed, only copied.
const EXECUTABLE_CONTENT: &[u8] = b"not really an executable";

#[test]
fn test_install_files() {
    let root = tempdir().unwrap();
    let source_exe = root.path().join("moonwatch_rs-download");
    fs::write(&source_exe, EXECUTABLE_CONTENT).unwrap();

    // Not created beforehand: the installer is what creates the installation directory.
    let moonwatch_dir = root.path().join(".moonwatch-rs");
    install_files(&moonwatch_dir, &source_exe).unwrap();

    for name in INSTALLED_FILES {
        let path = moonwatch_dir.join(name);
        assert!(path.is_file(), "{} was not installed", path.display());
    }

    assert_eq!(fs::read(moonwatch_dir.join(EXECUTABLE_NAME)).unwrap(), EXECUTABLE_CONTENT,
               "the executable was not copied under its canonical name");
}

/// Re-running `install` is the upgrade path, and an upgrade must not undo the user's work.
#[test]
fn test_install_files_keeps_edited_configs() {
    let root = tempdir().unwrap();
    let source_exe = root.path().join("moonwatch_rs-download");
    fs::write(&source_exe, EXECUTABLE_CONTENT).unwrap();

    let moonwatch_dir = root.path().join(".moonwatch-rs");
    install_files(&moonwatch_dir, &source_exe).unwrap();

    let main_config = moonwatch_dir.join("main_config.json");
    let edited = fs::read_to_string(&main_config).unwrap()
        .replace("\"sampleEverySec\": 15", "\"sampleEverySec\": 60");
    fs::write(&main_config, &edited).unwrap();

    install_files(&moonwatch_dir, &source_exe).unwrap();

    assert_eq!(fs::read_to_string(&main_config).unwrap(), edited,
               "an edited configuration was overwritten by a second install");
}

/// `moonwatch_rs install` run from an existing installation copies the executable onto
/// itself, which would truncate it if it were done naively.
#[test]
fn test_install_files_from_the_installation_itself() {
    let root = tempdir().unwrap();
    let moonwatch_dir = root.path().join(".moonwatch-rs");
    fs::create_dir_all(&moonwatch_dir).unwrap();

    let installed_exe = moonwatch_dir.join(EXECUTABLE_NAME);
    fs::write(&installed_exe, EXECUTABLE_CONTENT).unwrap();

    install_files(&moonwatch_dir, &installed_exe).unwrap();

    assert_eq!(fs::read(&installed_exe).unwrap(), EXECUTABLE_CONTENT,
               "the installed executable was destroyed by copying it onto itself");
}
