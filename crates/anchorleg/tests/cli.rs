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
        .env(
            "ANCHORLEG_NOTIFY_LOG",
            config.parent().unwrap().join("notify.log"),
        )
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
         claude-personal='CLAUDE_CONFIG_DIR=~/.claude-personal /Users/me/.local/bin/claude'\n",
    )
    .unwrap();

    let preview = anchorleg(&config)
        .args(["accounts", "import-aliases", "--from-file"])
        .arg(&aliases)
        .assert()
        .success();
    assert!(
        String::from_utf8_lossy(&preview.get_output().stdout)
            .contains("would add  claude-personal")
    );
    assert!(!config.exists());

    anchorleg(&config)
        .args(["accounts", "import-aliases", "--yes", "--from-file"])
        .arg(&aliases)
        .assert()
        .success();
    anchorleg(&config)
        .args(["accounts", "show", "claude-personal"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "CLAUDE_CONFIG_DIR=/Users/me/.claude-personal /Users/me/.local/bin/claude",
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
    for (name, prio) in [("personal", "1"), ("two", "2")] {
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
    assert_eq!(v["next"]["account"], "personal");

    // personal is out until far in the future; two's weekly window is at 40%.
    anchorleg(&config)
        .args(["report", "--json"])
        .write_stdin(
            r#"{"account":"personal","readings":[{"window":"five_hour","status":"rejected","used":1.0,"resets_at":99999999999}]}"#,
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
        .args(["accounts", "add", "personal", "--cmd", "claude"])
        .assert()
        .success();
    for bad in [
        "not json",
        r#"{"account":"ghost","readings":[]}"#,
        r#"{"account":"personal","readings":[{"window":"five_hour","used":91}]}"#,
        r#"{"account":"personal","readings":[],"extra":1}"#,
    ] {
        anchorleg(&config)
            .args(["report", "--json"])
            .write_stdin(bad)
            .assert()
            .failure();
    }
}

#[test]
fn notify_settings() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    anchorleg(&config)
        .args(["notify"])
        .assert()
        .success()
        .stdout("desktop  on\ncommand  (none)\n");
    anchorleg(&config)
        .args([
            "notify",
            "--desktop",
            "off",
            "--command",
            "say done",
            "--test",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "desktop  off\ncommand  say done\nsent a test notice",
        ));
    let saved = std::fs::read_to_string(&config).unwrap();
    assert!(
        saved.contains("[notify]") && saved.contains("desktop = false"),
        "{saved}"
    );
    let log = std::fs::read_to_string(dir.path().join("notify.log")).unwrap();
    assert!(log.contains(r#""event":"test""#), "{log}");
    anchorleg(&config)
        .args(["notify", "--desktop", "on", "--no-command"])
        .assert()
        .success();
    // Back to the defaults: nothing left in the file.
    assert!(!std::fs::read_to_string(&config).unwrap().contains("notify"));
}

#[test]
fn settings_rules_policy_and_aliases_in_json() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let json = |args: &[&str]| -> serde_json::Value {
        let out = anchorleg(&config).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };

    let s = json(&["settings", "claude", "--model", "opus", "--json"]);
    assert_eq!(s["vendors"]["claude"]["model"], "opus", "{s}");
    assert_eq!(
        s["vendors"]["codex"]["model"],
        serde_json::Value::Null,
        "{s}"
    );
    let s = json(&["settings", "--json"]);
    assert_eq!(s["vendors"]["claude"]["model"], "opus", "{s}");

    let r = json(&["rules", "--add", "Bash(cargo test *)", "--json"]);
    assert_eq!(r["rules"], serde_json::json!(["Bash(cargo test *)"]), "{r}");
    let r = json(&["rules", "--rm", "Bash(cargo test *)", "--json"]);
    assert_eq!(r["rules"], serde_json::json!([]), "{r}");

    let p = json(&["policy", "balance", "--json"]);
    assert_eq!(p["policy"], "balance", "{p}");
    assert_eq!(p["policies"].as_array().unwrap().len(), 3, "{p}");

    let aliases = dir.path().join("aliases.txt");
    std::fs::write(
        &aliases,
        "claude-work='CLAUDE_CONFIG_DIR=~/.claude-work /Users/me/.local/bin/claude'\n",
    )
    .unwrap();
    let from = aliases.to_str().unwrap();
    let a = json(&["accounts", "import-aliases", "--json", "--from-file", from]);
    assert_eq!(a["add"][0]["name"], "claude-work", "{a}");
    assert_eq!(a["saved"], false, "{a}");
    let a = json(&[
        "accounts",
        "import-aliases",
        "--yes",
        "--json",
        "--from-file",
        from,
    ]);
    assert_eq!(a["saved"], true, "{a}");
    let a = json(&["accounts", "import-aliases", "--json", "--from-file", from]);
    assert_eq!(a["skip"][0], "claude-work", "{a}");
    assert_eq!(a["add"], serde_json::json!([]), "{a}");
}

#[test]
fn login_signs_in_then_adds_the_account() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake/fake-claude.sh");

    // A sign-in that doesn't finish adds nothing.
    anchorleg(&config)
        .env("HOME", &home)
        .env("FAKE_LOGIN_FAILS", "1")
        .args(["accounts", "login", "work", "--bin", fake])
        .assert()
        .failure()
        .stderr(predicates::str::contains("nothing was added"));
    anchorleg(&config)
        .args(["accounts", "list", "--json"])
        .assert()
        .success()
        .stdout(predicates::prelude::PredicateBooleanExt::not(
            predicates::str::contains("work"),
        ));

    let out = anchorleg(&config)
        .env("HOME", &home)
        .args(["accounts", "login", "work", "--bin", fake, "--json"])
        .assert()
        .success();
    let v = json(&out);
    assert_eq!(v["name"], "work");
    assert_eq!(v["cli"], "claude");
    assert_eq!(v["dir"], "~/.claude-work");
    assert!(home.join(".claude-work/.fake-signed-in").is_file());
    let list = json(
        &anchorleg(&config)
            .args(["accounts", "list", "--json"])
            .assert()
            .success(),
    );
    assert_eq!(list[0]["name"], "work");
    assert_eq!(list[0]["env"]["CLAUDE_CONFIG_DIR"], "~/.claude-work");

    // The same name again, a bad name, and a CLI it can't sign in to are refused.
    for args in [
        &["accounts", "login", "work", "--bin", fake][..],
        &["accounts", "login", "a b", "--bin", fake][..],
        &["accounts", "login", "g", "--cli", "kimi"][..],
    ] {
        anchorleg(&config)
            .env("HOME", &home)
            .args(args)
            .assert()
            .failure();
    }
}

#[test]
fn login_to_antigravity_needs_a_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // Not a terminal here: agy's sign-in screen can't run, so it says where to do it.
    anchorleg(&config)
        .env("HOME", &home)
        .args(["accounts", "login", "g2", "--cli", "agy"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("in a terminal"));
}
