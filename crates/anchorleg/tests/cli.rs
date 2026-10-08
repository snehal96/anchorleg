use assert_cmd::Command;

#[test]
fn version_matches_crate() {
    Command::cargo_bin("anchorleg")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("anchorleg {}\n", anchorleg_core::VERSION));
}

#[test]
fn run_needs_a_prompt() {
    Command::cargo_bin("anchorleg")
        .unwrap()
        .arg("run")
        .assert()
        .code(2);
}

fn anchorleg(config: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("anchorleg").unwrap();
    let home = config.parent().unwrap().join("anchorleg-home");
    cmd.env("ANCHORLEG_CONFIG", config)
        .env("ANCHORLEG_HOME", home)
        .env("HOME", "/Users/me");
    cmd
}

#[test]
fn import_aliases_previews_then_writes() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let aliases = dir.path().join("aliases.txt");
    std::fs::write(
        &aliases,
        "claude='echo use specific commands'\n\
         claude-sm='CLAUDE_CONFIG_DIR=~/.claude-sm /Users/me/.local/bin/claude'\n",
    )
    .unwrap();

    let preview = anchorleg(&config)
        .args(["accounts", "import-aliases", "--from-file"])
        .arg(&aliases)
        .assert()
        .success();
    assert!(String::from_utf8_lossy(&preview.get_output().stdout).contains("would add  claude-sm"));
    assert!(!config.exists());

    anchorleg(&config)
        .args(["accounts", "import-aliases", "--yes", "--from-file"])
        .arg(&aliases)
        .assert()
        .success();
    anchorleg(&config)
        .args(["accounts", "show", "claude-sm"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "CLAUDE_CONFIG_DIR=/Users/me/.claude-sm /Users/me/.local/bin/claude",
        ));
}

#[test]
fn add_with_cmd_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    anchorleg(&config)
        .args([
            "accounts",
            "add",
            "two",
            "--priority",
            "2",
            "--cmd",
            "CLAUDE_CONFIG_DIR=~/.claude-two ~/.local/bin/claude",
        ])
        .assert()
        .success();
    anchorleg(&config)
        .args(["accounts", "add", "two", "--cmd", "claude"])
        .assert()
        .failure();
    anchorleg(&config)
        .args(["accounts", "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("two"));
    anchorleg(&config)
        .args(["accounts", "rm", "two"])
        .assert()
        .success();
    anchorleg(&config)
        .args(["accounts", "rm", "two"])
        .assert()
        .failure();
}

fn json(out: &assert_cmd::assert::Assert) -> serde_json::Value {
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

#[test]
fn report_then_status() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    for (name, prio) in [("sm", "1"), ("two", "2")] {
        anchorleg(&config)
            .args([
                "accounts",
                "add",
                name,
                "--priority",
                prio,
                "--cmd",
                "claude",
            ])
            .assert()
            .success();
    }

    let empty = anchorleg(&config)
        .args(["status", "--json"])
        .assert()
        .success();
    let v = json(&empty);
    assert_eq!(v["schema"], 1);
    assert_eq!(v["next"]["action"], "use");
    assert_eq!(v["next"]["account"], "sm");

    // sm is out until far in the future; two's weekly window is at 40%.
    anchorleg(&config)
        .args(["report", "--json"])
        .write_stdin(
            r#"{"account":"sm","readings":[{"window":"five_hour","status":"rejected","used":1.0,"resets_at":99999999999}]}"#,
        )
        .assert()
        .success();
    anchorleg(&config)
        .args(["report", "--json"])
        .write_stdin(r#"{"account":"two","readings":[{"window":"seven_day","used":0.4}]}"#)
        .assert()
        .success();

    let v = json(
        &anchorleg(&config)
            .args(["status", "--json"])
            .assert()
            .success(),
    );
    assert_eq!(v["next"]["account"], "two");
    assert_eq!(v["accounts"][0]["blocked_until"], 99999999999_i64);
    assert_eq!(v["accounts"][1]["windows"][0]["used"], 0.4);
    assert_eq!(v["accounts"][1]["windows"][0]["source"], "mod");

    anchorleg(&config)
        .arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains("next run: use two"));
}

#[test]
fn report_rejects_bad_input() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    anchorleg(&config)
        .args(["accounts", "add", "sm", "--cmd", "claude"])
        .assert()
        .success();
    for bad in [
        "not json",
        r#"{"account":"ghost","readings":[]}"#,
        r#"{"account":"sm","readings":[{"window":"five_hour","used":91}]}"#,
        r#"{"account":"sm","readings":[],"extra":1}"#,
    ] {
        anchorleg(&config)
            .args(["report", "--json"])
            .write_stdin(bad)
            .assert()
            .failure();
    }
}
