use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
fn fixture_path(temp_dir: &TempDir, file_name: &str) -> PathBuf {
    temp_dir.path().join(file_name)
}
fn write_fixture(temp_dir: &TempDir, file_name: &str, content: &str) -> PathBuf {
    let path = fixture_path(temp_dir, file_name);
    fs::write(&path, content).unwrap();
    path
}
fn run_cli(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-flow")).args(args).output().unwrap()
}
#[test]
fn expands_features_and_versions_via_cli() {
    let temp_dir = TempDir::new().unwrap();
    let manifest_path = write_fixture(
        &temp_dir,
        "Cargo.toml",
        "[package]\nname = \"demo\"\nedition.workspace = true\n\n[dependencies]\nserde.workspace = true\nserde_json = { version = \"1.0.0\", features = [] }\ntoml = \"0.2.0\"\n",
    );
    let output = run_cli(&[manifest_path.as_os_str(), std::ffi::OsStr::new("--features"), std::ffi::OsStr::new("--normalize-versions")]);
    assert!(output.status.success());
    let stdout_text = String::from_utf8(output.stdout).unwrap();
    let expanded_line = format!("expanded 2 dotted keys in {}", manifest_path.display());
    let filled_line = format!("filled 2 features keys in {}", manifest_path.display());
    let normalized_line = format!("normalized 2 versions in {}", manifest_path.display());
    assert!(stdout_text.contains(&expanded_line), "stdout: {stdout_text}");
    assert!(stdout_text.contains(&filled_line), "stdout: {stdout_text}");
    assert!(stdout_text.contains(&normalized_line), "stdout: {stdout_text}");
    let manifest_text = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        manifest_text,
        "[package]\nname = \"demo\"\nedition = { workspace = true }\n\n[dependencies]\nserde = { workspace = true, features = [] }\nserde_json = { version = \"1\", features = [] }\ntoml = { version = \"0.2\", features = [] }\n"
    );
}
#[test]
fn second_run_is_silent_and_byte_identical() {
    let temp_dir = TempDir::new().unwrap();
    let manifest_path = write_fixture(
        &temp_dir,
        "Cargo.toml",
        "[package]\nname = \"demo\"\nedition.workspace = true\n\n[dependencies]\nserde.workspace = true\ntoml = \"0.2.0\"\n",
    );
    let first_output = run_cli(&[manifest_path.as_os_str(), std::ffi::OsStr::new("--features"), std::ffi::OsStr::new("--normalize-versions")]);
    assert!(first_output.status.success());
    let content_after_first_run = fs::read(&manifest_path).unwrap();
    let second_output = run_cli(&[manifest_path.as_os_str(), std::ffi::OsStr::new("--features"), std::ffi::OsStr::new("--normalize-versions")]);
    assert!(second_output.status.success());
    assert!(second_output.stdout.is_empty(), "stdout: {}", String::from_utf8(second_output.stdout).unwrap());
    assert_eq!(fs::read(&manifest_path).unwrap(), content_after_first_run);
}
#[test]
fn reports_conflict_as_fatal_with_exit_one() {
    let temp_dir = TempDir::new().unwrap();
    let manifest_path = write_fixture(&temp_dir, "Cargo.toml", "[dependencies]\nfoo = { x = 1 }\nfoo.y = 2\n");
    let output = run_cli(&[manifest_path.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr_text = String::from_utf8(output.stderr).unwrap();
    assert!(stderr_text.contains("foo"), "stderr: {stderr_text}");
    assert_eq!(fs::read_to_string(&manifest_path).unwrap(), "[dependencies]\nfoo = { x = 1 }\nfoo.y = 2\n");
}
#[test]
fn reports_parse_error_as_fatal() {
    let temp_dir = TempDir::new().unwrap();
    let manifest_path = write_fixture(&temp_dir, "Cargo.toml", "[dependencies\n");
    let output = run_cli(&[manifest_path.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
}
#[test]
fn resolves_directory_argument_to_cargo_toml() {
    let temp_dir = TempDir::new().unwrap();
    let manifest_path = write_fixture(&temp_dir, "Cargo.toml", "[dependencies]\nserde.workspace = true\n");
    let output = run_cli(&[temp_dir.path().as_os_str()]);
    assert!(output.status.success());
    let stdout_text = String::from_utf8(output.stdout).unwrap();
    let expanded_line = format!("expanded 1 dotted keys in {}", manifest_path.display());
    assert!(stdout_text.contains(&expanded_line), "stdout: {stdout_text}");
    assert_eq!(fs::read_to_string(&manifest_path).unwrap(), "[dependencies]\nserde = { workspace = true }\n");
}
#[test]
fn is_silent_for_unchanged_file() {
    let temp_dir = TempDir::new().unwrap();
    let manifest_path = write_fixture(
        &temp_dir,
        "Cargo.toml",
        "[package]\nname = \"demo\"\nedition = { workspace = true }\n\n[dependencies]\nserde = { workspace = true, features = [] }\nserde_json = { version = \"1\", features = [] }\n",
    );
    let output = run_cli(&[manifest_path.as_os_str(), std::ffi::OsStr::new("--features"), std::ffi::OsStr::new("--normalize-versions")]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "stdout: {}", String::from_utf8(output.stdout).unwrap());
}
#[test]
fn reports_missing_file_as_fatal() {
    let temp_dir = TempDir::new().unwrap();
    let missing_path = fixture_path(&temp_dir, "absent.toml");
    let missing_path_ref: &Path = &missing_path;
    let output = run_cli(&[missing_path_ref.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr_text = String::from_utf8(output.stderr).unwrap();
    assert!(stderr_text.contains("absent.toml"), "stderr: {stderr_text}");
}
