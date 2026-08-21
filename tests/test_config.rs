use std::fs;
use uuid::Uuid;
use moonwatch_rs::core::config_writer::ConfigWriter;

#[test]
fn test_write_default_config() {
    let output_dir = std::env::temp_dir().join(format!("moonwatch-test-{}", Uuid::now_v7()));
    fs::create_dir_all(output_dir.clone()).unwrap();

    let writer = ConfigWriter::new(output_dir.as_path());
    writer.write_schemas().unwrap();
    writer.write_default_configs(false).unwrap();

    fs::remove_dir_all(&output_dir).ok();
}
