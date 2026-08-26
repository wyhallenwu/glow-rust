use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn glow() -> Command {
    Command::new(env!("CARGO_BIN_EXE_glow"))
}

#[test]
fn help_exposes_the_browser_and_share_workflows() {
    glow().arg("--help").assert().success().stdout(
        predicate::str::contains("serve")
            .and(predicate::str::contains("share"))
            .and(predicate::str::contains("SOURCE|DIR")),
    );

    glow()
        .args(["serve", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("-P, --port"));
}

#[test]
fn list_reports_nested_markdown_as_json() {
    let temp = tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("guide/deep")).expect("create fixture folders");
    std::fs::write(temp.path().join("README.md"), "# Home").expect("write README");
    std::fs::write(temp.path().join("guide/deep/api.md"), "# API").expect("write API doc");
    std::fs::write(temp.path().join("guide/deep/code.rs"), "fn main() {}").expect("write code");

    glow()
        .args([
            "list",
            temp.path().to_str().expect("UTF-8 temp path"),
            "--json",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("README.md")
                .and(predicate::str::contains("guide/deep/api.md"))
                .and(predicate::str::contains("code.rs").not()),
        );
}

#[test]
fn piped_markdown_renders_without_ansi_when_captured() {
    glow()
        .write_stdin("# Hello\n\n> from stdin\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("▌ Hello"))
        .stdout(predicate::str::contains("│ from stdin"))
        .stdout(predicate::str::contains("\u{1b}[").not());
}

#[test]
fn config_init_can_create_an_explicit_path() {
    let temp = tempdir().expect("tempdir");
    let config = temp.path().join("nested/custom.yml");

    glow()
        .arg("--config")
        .arg(&config)
        .args(["config", "init"])
        .assert()
        .success();

    let written = std::fs::read_to_string(config).expect("read generated config");
    assert!(written.contains("style: auto"));
    assert!(written.contains("showLineNumbers: false"));
}
