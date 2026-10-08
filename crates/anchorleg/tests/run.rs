//! End-to-end `anchorleg run` against the fake CLIs in `tests/fake/` (Claude, Codex, agy).
//! Spends no quota.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

const FAR: i64 = 99_999_999_999;

fn init(sid: &str) -> String {
    format!(
        r#"{{"type":"system","subtype":"init","session_id":"{sid}","model":"claude-haiku-5-5"}}"#
    )
}

fn ok(sid: &str, text: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        init(sid),
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1791466800,"rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.2,"resetsAt":1791466800}}}}"#,
        format_args!(
            r#"{{"type":"result","subtype":"success","is_error":false,"result":"{text}","session_id":"{sid}"}}"#
        )
    )
}

fn limit(sid: &str, resets_at: i64) -> String {
    format!(
        "{}\n{}\n{}\n",
        init(sid),
        format_args!(
            r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","resetsAt":{resets_at},"rateLimitType":"five_hour","utilization":1.0}}}}"#
        ),
        format_args!(
            r#"{{"type":"result","subtype":"success","is_error":true,"result":"Claude AI usage limit reached|{resets_at}","session_id":"{sid}"}}"#
        )
    )
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    /// Claude accounts in priority order (first is 1), each with its own config dir.
    fn new(accounts: &[&str]) -> Self {
        let mixed: Vec<(&str, &str)> = accounts.iter().map(|a| (*a, "claude")).collect();
        Self::mixed(&mixed)
    }

    /// `(name, vendor)` accounts in priority order; each has its own config dir / `CODEX_HOME`.
    fn mixed(accounts: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = String::new();
        for (i, (name, vendor)) in accounts.iter().enumerate() {
            let cfg = dir.path().join("cfg").join(name);
            std::fs::create_dir_all(&cfg).unwrap();
            let (fake, var) = match *vendor {
                "codex" => ("fake-codex.sh", "CODEX_HOME"),
                "antigravity" => ("fake-agy.sh", "HOME"),
                _ => ("fake-claude.sh", "CLAUDE_CONFIG_DIR"),
            };
            let fake = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fake")
                .join(fake);
            config += &format!(
                "[[account]]\nname = \"{name}\"\nvendor = \"{vendor}\"\npriority = {}\nbin = \"{}\"\nenv = {{ {var} = \"{}\" }}\n\n",
                i + 1,
                fake.display(),
                cfg.display()
            );
        }
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        std::fs::create_dir_all(dir.path().join("fake")).unwrap();
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        Self { dir }
    }

    fn script(&self, file: &str, body: &str) {
        std::fs::write(self.dir.path().join("fake").join(file), body).unwrap();
    }

    fn path(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }

    fn anchorleg(&self) -> Command {
        let mut cmd = Command::cargo_bin("anchorleg").unwrap();
        cmd.env("ANCHORLEG_CONFIG", self.path("config.toml"))
            .env("ANCHORLEG_HOME", self.path("home"))
            .env("HOME", self.path("userhome"))
            .env("FAKE_DIR", self.path("fake"))
            .env("FAKE_LOG", self.path("fake.log"))
            .env("CLAUDE_CONFIG_DIR", "/should/be/scrubbed")
            .env("CODEX_HOME", "/should/be/scrubbed")
            .env("ANTHROPIC_API_KEY", "should-be-scrubbed");
        cmd
    }

    fn run(&self, extra: &[&str]) -> (i32, Value) {
        self.run_with(self.anchorleg(), extra)
    }

    fn run_with(&self, mut cmd: Command, extra: &[&str]) -> (i32, Value) {
        let out = cmd
            .arg("run")
            .arg("--json")
            .arg("--cwd")
            .arg(self.path("work"))
            .args(extra)
            .args(["--", "fix", "the", "tests"])
            .output()
            .unwrap();
        let report = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "bad JSON ({e}): {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), report)
    }

    fn launches(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("fake.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn finishes_on_first_account() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &ok("s1", "all green"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0);
    assert_eq!(r["outcome"], "done");
    assert_eq!(r["result"], "all green");
    assert_eq!(r["accounts_used"], serde_json::json!(["a"]));

    let launches = env.launches();
    assert_eq!(launches.len(), 1);
    assert!(launches[0].starts_with("a --plugin-dir "));
    assert!(launches[0].contains(" -p fix the tests --output-format stream-json --verbose"));
    assert!(launches[0].ends_with("[stop_at=0.9]"));
}

#[test]
fn limit_hit_switches_and_resumes_the_same_session() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &limit("s1", FAR));
    env.script("b.jsonl", &ok("s1", "finished on b"));

    env.anchorleg()
        .args([
            "settings",
            "claude",
            "--args",
            "--permission-mode acceptEdits",
        ])
        .assert()
        .success();
    let (code, r) = env.run(&["--model", "haiku"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["outcome"], "done");
    assert_eq!(r["result"], "finished on b");
    assert_eq!(r["session_id"], "s1");
    assert_eq!(r["accounts_used"], serde_json::json!(["a", "b"]));
    assert_eq!(r["switches"][0]["from"], "a");
    assert_eq!(r["switches"][0]["to"], "b");
    assert_eq!(r["switches"][0]["rule"], "rejected");

    let launches = env.launches();
    assert_eq!(launches.len(), 2);
    // `[vendor.claude]` args come first, then anchorleg's own.
    assert!(launches[1].starts_with("b --permission-mode acceptEdits --plugin-dir"));
    assert!(launches[1].contains(" -p Continue from where you stopped."));
    assert!(launches[1].contains("--model haiku --resume s1"));
    // The session file and project memory were copied into b's config dir first (D11, D14).
    assert!(env.path("cfg/b/projects/-fake/s1.jsonl").is_file());
    assert!(env.path("cfg/b/projects/-fake/memory/notes.md").is_file());

    // a stays blocked for the next run.
    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["accounts"][0]["blocked_until"], FAR);
    assert_eq!(status["next"]["account"], "b");
}

#[test]
fn unresumable_session_falls_back_to_a_fresh_handoff() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &limit("s1", FAR));
    env.script("b.jsonl", &ok("s2", "redone on b"));
    // No session files anywhere: the copy finds nothing and b can't resume.
    let mut cmd = env.anchorleg();
    cmd.env("FAKE_NO_SESSION_FILE", "1");

    let (code, r) = env.run_with(cmd, &[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "redone on b");

    let launches = env.launches();
    assert_eq!(launches.len(), 3, "{launches:?}");
    assert!(launches[1].contains("--resume"));
    assert!(launches[2].contains("You are taking over a task"));
    assert!(launches[2].contains("# Task handoff"));
    assert!(!launches[2].contains("--resume"));
}

#[test]
fn all_blocked_with_no_wait_exits_75() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &limit("s1", FAR));
    env.script("b.jsonl", &limit("s1", FAR - 1000));

    let (code, r) = env.run(&["--no-wait"]);
    assert_eq!(code, 75, "{r}");
    assert_eq!(r["outcome"], "all_blocked");
    assert_eq!(r["wait_until"], FAR - 1000);
    assert_eq!(env.launches().len(), 2);
}

#[test]
fn task_failure_exits_1() {
    let env = Env::new(&["a"]);
    env.script(
        "a.jsonl",
        &format!(
            "{}\n{}\n",
            init("s1"),
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"ran out of turns","session_id":"s1"}"#
        ),
    );
    let (code, r) = env.run(&[]);
    assert_eq!(code, 1);
    assert_eq!(r["outcome"], "failed");
    assert_eq!(r["result"], "ran out of turns");
}

#[test]
fn no_accounts_exits_78() {
    let env = Env::new(&[]);
    let (code, r) = env.run(&[]);
    assert_eq!(code, 78);
    assert_eq!(r["outcome"], "no_accounts");
}

#[test]
fn mod_stop_switches_at_a_clean_point() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &ok("s1", "[anchorleg] Pausing here"));
    env.script("b.jsonl", &ok("s1", "finished on b"));
    let mut cmd = env.anchorleg();
    cmd.env("FAKE_MOD_STOP", "a");

    let (code, r) = env.run_with(cmd, &[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on b");
    assert_eq!(r["switches"][0]["rule"], "quota_stop");
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(launches[1].starts_with("b "));
    assert!(launches[1].contains("--resume s1"));
    assert!(env.path("cfg/b/projects/-fake/s1.jsonl").is_file());

    // a isn't blocked: a soft stop leaves it usable once others run low too.
    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["accounts"][0]["blocked_until"], Value::Null);
}

#[test]
fn mod_stop_with_nowhere_better_continues_with_the_stop_off() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "[anchorleg] Pausing here"));
    env.script("a.2.jsonl", &ok("s1", "finished on a"));
    let mut cmd = env.anchorleg();
    cmd.env("FAKE_MOD_STOP", "a");

    let (code, r) = env.run_with(cmd, &[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on a");
    assert_eq!(r["switches"], serde_json::json!([]));
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(launches[1].contains("--resume s1"));
    assert!(launches[1].ends_with("[stop_at=0]"));
}

#[test]
fn no_mod_flag_runs_without_the_mod() {
    let env = Env::new(&["a"]);
    env.script("a.jsonl", &ok("s1", "done"));
    let (code, _) = env.run(&["--no-mod"]);
    assert_eq!(code, 0);
    assert!(!env.launches()[0].contains("--plugin-dir"));
}

#[test]
fn follow_up_sends_a_message_into_the_same_session() {
    let env = Env::new(&["a", "b"]);
    env.script("a.1.jsonl", &ok("s1", "tests fixed"));
    env.script("a.2.jsonl", &ok("s1", "docs added"));
    assert_eq!(env.run(&[]).0, 0);

    let (code, r) = env.run(&["--follow-up", "1"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "docs added");
    let launches = env.launches();
    assert!(launches[1].starts_with("a "));
    assert!(launches[1].contains(" -p fix the tests --output-format stream-json --verbose"));
    assert!(launches[1].contains("--resume s1"));

    let sessions = env
        .anchorleg()
        .args(["sessions", "--json"])
        .output()
        .unwrap();
    let sessions: Value = serde_json::from_slice(&sessions.stdout).unwrap();
    assert_eq!(sessions[0]["parent_id"], 1);
    assert_eq!(sessions[0]["pid"], Value::Null);
}

#[test]
fn follow_up_on_another_account_carries_the_context() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &ok("s1", "first answer"));
    env.script("b.jsonl", &ok("s1", "answer from b"));
    assert_eq!(env.run(&[]).0, 0);
    // a runs out between messages.
    env.anchorleg()
        .args(["report", "--json"])
        .write_stdin(format!(
            r#"{{"account":"a","readings":[{{"window":"five_hour","status":"rejected","resets_at":{FAR}}}]}}"#
        ))
        .assert()
        .success();

    let (code, r) = env.run(&["--follow-up", "1"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "answer from b");
    assert!(env.launches()[1].starts_with("b "));
    assert!(env.launches()[1].contains("--resume s1"));
    assert!(env.path("cfg/b/projects/-fake/s1.jsonl").is_file());
}

#[test]
fn stop_ends_a_running_session() {
    use std::os::unix::process::CommandExt as _;
    let env = Env::new(&["a"]);
    env.script("a.jsonl", &ok("s1", "never reached"));
    let bin = assert_cmd::cargo::cargo_bin("anchorleg");
    let mut child = std::process::Command::new(&bin)
        .args(["run", "--json", "--cwd"])
        .arg(env.path("work"))
        .args(["--", "long", "task"])
        .env("ANCHORLEG_CONFIG", env.path("config.toml"))
        .env("ANCHORLEG_HOME", env.path("home"))
        .env("HOME", env.path("userhome"))
        .env("FAKE_DIR", env.path("fake"))
        .env("FAKE_LOG", env.path("fake.log"))
        .env("FAKE_SLEEP", "30")
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // Wait until the run has started its session.
    let started = std::time::Instant::now();
    loop {
        let out = env
            .anchorleg()
            .args(["sessions", "--json"])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        if v[0]["session_id"] == "s1" {
            break;
        }
        assert!(started.elapsed().as_secs() < 10, "run never started");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    env.anchorleg().args(["stop", "1"]).assert().success();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    assert!(started.elapsed().as_secs() < 15, "stop took too long");

    let out = env
        .anchorleg()
        .args(["sessions", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v[0]["state"], "stopped");
    assert_eq!(v[0]["pid"], Value::Null);
    env.anchorleg().args(["stop", "1"]).assert().failure();
}

/// An assistant turn that edits a file, runs a command and says something, with this much
/// context in its usage.
fn working(sid: &str, context: u64) -> String {
    format!(
        "{}\n{}\n",
        init(sid),
        format_args!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Half the tests pass."}},{{"type":"tool_use","id":"t1","name":"Edit","input":{{"file_path":"src/lib.rs"}}}},{{"type":"tool_use","id":"t2","name":"Bash","input":{{"command":"cargo test"}}}}],"usage":{{"input_tokens":{context}}}}},"session_id":"{sid}"}}"#
        )
    )
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn limit_hit_writes_a_handoff_and_a_checkpoint() {
    let env = Env::new(&["a", "b"]);
    let work = env.path("work");
    git(&work, &["init", "-q"]);
    std::fs::write(work.join("lib.rs"), "v1\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "init"]);
    std::fs::write(work.join("lib.rs"), "v2 half done\n").unwrap();
    let head = git(&work, &["rev-parse", "HEAD"]);

    env.script("a.jsonl", &(working("s1", 2_000) + &limit("s1", FAR)));
    env.script("b.jsonl", &ok("s1", "finished on b"));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");

    // Small context: b resumes the session.
    let launches = env.launches();
    assert!(launches[1].contains("--resume s1"), "{launches:?}");

    let md = std::fs::read_to_string(work.join(".handoff/TASK.md")).unwrap();
    assert!(md.contains("## Goal\n\nfix the tests"), "{md}");
    assert!(md.contains("> Half the tests pass."), "{md}");
    assert!(md.contains("- `src/lib.rs` (edited)"), "{md}");
    assert!(md.contains("- `cargo test`"), "{md}");
    assert!(md.contains(" M lib.rs"), "{md}");
    assert!(md.contains("refs/anchorleg/run-1/1"), "{md}");

    // The checkpoint holds the edit; the user's branch, index and files are untouched.
    assert_eq!(
        git(&work, &["show", "refs/anchorleg/run-1/1:lib.rs"]),
        "v2 half done"
    );
    assert_eq!(git(&work, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&work, &["status", "--short"]), "M lib.rs");
}

#[test]
fn large_context_starts_fresh_from_the_handoff() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &(working("s1", 150_000) + &limit("s1", FAR)));
    env.script("b.jsonl", &ok("s2", "redone on b"));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["switches"][0]["rule"], "rejected");

    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(!launches[1].contains("--resume"), "{launches:?}");
    assert!(launches[1].contains("You are taking over a task"));
    assert!(launches[1].contains("> Half the tests pass."));
    // No git repo here: still a handoff, just no checkpoint.
    let md = std::fs::read_to_string(env.path("work/.handoff/TASK.md")).unwrap();
    assert!(!md.contains("## Checkpoint"), "{md}");

    // A higher threshold resumes instead.
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &(working("s1", 150_000) + &limit("s1", FAR)));
    env.script("b.jsonl", &ok("s1", "resumed on b"));
    let (code, r) = env.run(&["--fresh-above", "200000"]);
    assert_eq!(code, 0, "{r}");
    assert!(env.launches()[1].contains("--resume s1"));
}

#[test]
fn a_tool_call_waits_for_the_users_answer() {
    let env = Env::new(&["a"]);
    env.script("a.jsonl", &ok("s1", "wrote it"));
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("anchorleg"));
    let child = cmd
        .args(["run", "--json", "--cwd"])
        .arg(env.path("work"))
        .args(["--", "write", "a.txt"])
        .env("ANCHORLEG_CONFIG", env.path("config.toml"))
        .env("ANCHORLEG_HOME", env.path("home"))
        .env("HOME", env.path("userhome"))
        .env("FAKE_DIR", env.path("fake"))
        .env("FAKE_LOG", env.path("fake.log"))
        .env("FAKE_ASK", "1")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // The question shows up; answer it the way `anchorleg ui` does.
    let started = std::time::Instant::now();
    let id = loop {
        let out = env
            .anchorleg()
            .args(["permission", "list", "--json"])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        if let Some(id) = v[0]["id"].as_i64() {
            assert_eq!(v[0]["tool"], "Write");
            assert_eq!(v[0]["run_id"], 1);
            break id;
        }
        assert!(started.elapsed().as_secs() < 10, "nothing asked");
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    env.anchorleg()
        .args(["permission", "answer", &id.to_string(), "always"])
        .assert()
        .success();

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let answer: Value =
        serde_json::from_str(&std::fs::read_to_string(env.path("fake/ask.out")).unwrap()).unwrap();
    assert_eq!(answer["decision"], "allow");
    // Answered once: it can't be answered again.
    env.anchorleg()
        .args(["permission", "answer", &id.to_string(), "no"])
        .assert()
        .failure();
    // The conversation shows the question and the answer.
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(
        log.contains("waiting for you to allow Write  a.txt"),
        "{log}"
    );
    assert!(
        log.contains("Write  a.txt: allowed for this session"),
        "{log}"
    );
}

#[test]
fn provider_model_and_effort_apply_to_every_account() {
    let env = Env::new(&["a", "b"]);
    env.script("a.jsonl", &limit("s1", FAR));
    env.script("b.jsonl", &ok("s1", "done"));
    env.anchorleg()
        .args(["settings", "claude", "--model", "opus", "--effort", "high"])
        .assert()
        .success();
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    let launches = env.launches();
    for l in &launches {
        assert!(l.contains("--model opus --effort high"), "{l}");
    }
    // `anchorleg run --model` overrides the model for one run; effort stays shared.
    std::fs::remove_file(env.path("fake.log")).unwrap();
    let (code, _) = env.run(&["--model", "haiku"]);
    assert_eq!(code, 0);
    assert!(env.launches()[0].contains("--model haiku --effort high"));
    env.anchorleg()
        .args(["settings", "claude", "--effort", "huge"])
        .assert()
        .failure();
}

fn codex_ok(thread: &str, text: &str) -> String {
    format!(
        "{}\n{}\n{}\n{}\n",
        format_args!(r#"{{"type":"thread.started","thread_id":"{thread}"}}"#),
        r#"{"type":"turn.started"}"#,
        format_args!(
            r#"{{"type":"item.completed","item":{{"id":"item_0","type":"agent_message","text":"{text}"}}}}"#
        ),
        r#"{"type":"turn.completed","usage":{"input_tokens":1200,"cached_input_tokens":0,"output_tokens":10}}"#
    )
}

fn codex_limit(thread: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        format_args!(r#"{{"type":"thread.started","thread_id":"{thread}"}}"#),
        r#"{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc \"cargo test\"","aggregated_output":"","exit_code":0,"status":"completed"}}"#,
        r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Upgrade or try again later."}}"#
    )
}

fn codex_rollout(used_percent: f64, reached: bool) -> String {
    let reached = if reached {
        r#""rate_limit_reached""#
    } else {
        "null"
    };
    format!(
        r#"{{"type":"event_msg","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":{used_percent},"window_minutes":300,"resets_at":{FAR}}},"secondary":{{"used_percent":12.0,"window_minutes":10080,"resets_at":{FAR}}},"rate_limit_reached_type":{reached}}}}}}}"#
    ) + "\n"
}

#[test]
fn codex_runs_headless_and_reports_rollout_quota() {
    let env = Env::mixed(&[("c1", "codex")]);
    env.script("c1.jsonl", &codex_ok("t1", "all green"));
    env.script("c1.rollout.jsonl", &codex_rollout(30.0, false));
    env.anchorleg()
        .args([
            "settings",
            "codex",
            "--model",
            "gpt-5.6-luna",
            "--effort",
            "low",
        ])
        .assert()
        .success();

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "all green");
    assert_eq!(r["session_id"], "t1");
    let launches = env.launches();
    assert_eq!(
        launches,
        [
            r#"c1 exec --json --skip-git-repo-check -c sandbox_mode="workspace-write" -m gpt-5.6-luna -c model_reasoning_effort=low -- fix the tests"#
        ]
    );

    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    let windows = &status["accounts"][0]["windows"];
    assert_eq!(windows[0]["window"], "five_hour", "{status}");
    assert_eq!(windows[0]["used"], 0.3);
    assert_eq!(windows[1]["window"], "seven_day");

    // The conversation reads the Codex events in the run log.
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(log.contains(r#""type":"turn.completed""#));
}

#[test]
fn codex_limit_moves_the_thread_to_another_login() {
    let env = Env::mixed(&[("c1", "codex"), ("c2", "codex")]);
    env.script("c1.jsonl", &codex_limit("t1"));
    env.script("c1.rollout.jsonl", &codex_rollout(100.0, true));
    env.script("c2.jsonl", &codex_ok("t1", "finished on c2"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on c2");
    assert_eq!(r["accounts_used"], serde_json::json!(["c1", "c2"]));
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(
        launches[1].starts_with("c2 exec resume --json")
            && launches[1].ends_with("-- t1 Continue from where you stopped."),
        "{launches:?}"
    );
    assert!(
        env.path("cfg/c2/sessions/2026/10/08/rollout-2026-10-08T00-00-00-t1.jsonl")
            .is_file()
    );
    // The handoff read the Codex events.
    let task = std::fs::read_to_string(env.path("work/.handoff/TASK.md")).unwrap();
    assert!(task.contains("cargo test"), "{task}");

    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["accounts"][0]["blocked_until"], FAR, "{status}");
}

#[test]
fn claude_limit_continues_on_codex_from_the_handoff() {
    let env = Env::mixed(&[("a", "claude"), ("c", "codex")]);
    env.script("a.jsonl", &limit("s1", FAR));
    env.script("c.jsonl", &codex_ok("t9", "finished on codex"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on codex");
    assert_eq!(r["accounts_used"], serde_json::json!(["a", "c"]));
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(launches[1].starts_with("c exec --json"), "{launches:?}");
    assert!(!launches[1].contains("resume"));
    assert!(launches[1].contains("You are taking over a task"));
    assert!(launches[1].contains("# Task handoff"));
}

fn codex_limits(used_5h: u32, reached: bool) -> String {
    let reached = if reached {
        r#""rate_limit_reached""#
    } else {
        "null"
    };
    format!(
        r#"{{"rateLimits":{{"limitId":"codex","primary":{{"usedPercent":{used_5h},"windowDurationMins":300,"resetsAt":{FAR}}},"secondary":{{"usedPercent":10,"windowDurationMins":10080,"resetsAt":{FAR}}},"planType":"plus","rateLimitReachedType":{reached}}}}}"#
    )
}

#[test]
fn live_codex_quota_routes_to_the_login_with_room() {
    let env = Env::mixed(&[("c1", "codex"), ("c2", "codex")]);
    // c1 is first in line but out of quota right now; c2 has room. Nothing launches on c1.
    env.script("c1.limits.json", &codex_limits(100, true));
    env.script("c2.limits.json", &codex_limits(20, false));
    env.script("c1.jsonl", &codex_ok("t1", "ran on c1"));
    env.script("c2.jsonl", &codex_ok("t2", "ran on c2"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "ran on c2");
    assert_eq!(r["accounts_used"], serde_json::json!(["c2"]));
    assert!(env.launches().iter().all(|l| l.starts_with("c2 ")));
    let probes = std::fs::read_to_string(env.path("fake/probes.log")).unwrap();
    assert!(probes.contains("c1") && probes.contains("c2"), "{probes}");

    // c1's quota frees up: the next run goes back to it, and the old block is gone.
    env.script("c1.limits.json", &codex_limits(5, false));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "ran on c1");
    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(status["accounts"][0]["blocked_until"].is_null(), "{status}");
    assert_eq!(status["accounts"][0]["windows"][0]["used"], 0.05);

    // `status --refresh` reads it live too.
    env.script("c1.limits.json", &codex_limits(60, false));
    let status = env
        .anchorleg()
        .args(["status", "--json", "--refresh"])
        .output()
        .unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["accounts"][0]["windows"][0]["used"], 0.6, "{status}");
}

#[test]
fn codex_login_that_cant_report_quota_still_runs() {
    let env = Env::mixed(&[("c1", "codex")]);
    env.script("c1.nologin", "");
    env.script("c1.jsonl", &codex_ok("t1", "ran anyway"));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(
        log.contains("c1: couldn't read quota (codex account authentication required"),
        "{log}"
    );
}

fn agy_ok(id: &str, text: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        format_args!(
            r#"{{"event":"init","conversation_id":"{id}","init":{{"model":"gemini-3.8-flash-low","permission_mode":"request-review"}}}}"#
        ),
        format_args!(
            r#"{{"event":"step_update","step_update":{{"conversation_id":"{id}","step_index":2,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{{"name":"run_command","parameters":{{"CommandLine":"cargo test"}}}}}}}}"#
        ),
        format_args!(
            r#"{{"event":"result","result":{{"conversation_id":"{id}","status":"SUCCESS","response":"{text}\n","num_turns":1}}}}"#
        )
    )
}

fn agy_limit(id: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        format_args!(r#"{{"event":"init","conversation_id":"{id}","init":{{"model":"m"}}}}"#),
        format_args!(
            r#"{{"event":"step_update","step_update":{{"conversation_id":"{id}","step_index":2,"state":"DONE","step_type":"tool","tool_info":{{"name":"write_to_file","parameters":{{"TargetFile":"/w/half.rs"}}}}}}}}"#
        ),
        format_args!(
            r#"{{"event":"result","result":{{"conversation_id":"{id}","status":"ERROR","response":"","error":"RESOURCE_EXHAUSTED: You have exhausted your quota"}}}}"#
        )
    )
}

#[test]
fn agy_limit_moves_the_conversation_to_another_login() {
    let env = Env::mixed(&[("g1", "antigravity"), ("g2", "antigravity")]);
    env.script("g1.jsonl", &agy_limit("conv-1"));
    env.script("g2.jsonl", &agy_ok("conv-1", "finished on g2"));
    env.anchorleg()
        .args([
            "settings",
            "agy",
            "--model",
            "gemini-3.8-flash-low",
            "--effort",
            "low",
        ])
        .assert()
        .success();

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on g2");
    assert_eq!(r["session_id"], "conv-1");
    let launches = env.launches();
    assert_eq!(
        launches[0],
        "g1 -p fix the tests --output-format stream-json --mode accept-edits --model gemini-3.8-flash-low --effort low"
    );
    assert!(
        launches[1].starts_with("g2 -p Continue from where you stopped.")
            && launches[1].ends_with("--conversation conv-1"),
        "{launches:?}"
    );
    assert!(
        env.path("cfg/g2/.gemini/antigravity-cli/conversations/conv-1.db")
            .is_file()
    );
    let task = std::fs::read_to_string(env.path("work/.handoff/TASK.md")).unwrap();
    assert!(task.contains("/w/half.rs"), "{task}");
}

#[test]
fn claude_limit_continues_on_agy_from_the_handoff() {
    let env = Env::mixed(&[("a", "claude"), ("g", "antigravity")]);
    env.script("a.jsonl", &limit("s1", FAR));
    env.script("g.jsonl", &agy_ok("conv-9", "finished on agy"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on agy");
    let launches = env.launches();
    assert!(
        launches[1].starts_with("g -p You are taking over a task"),
        "{launches:?}"
    );
    assert!(!launches[1].contains("--conversation"));
}

#[test]
fn agy_follow_up_on_a_login_without_the_conversation_starts_fresh() {
    // A reply into conv-1 lands on g2, which doesn't have it: no silent new conversation,
    // anchorleg starts fresh from a handoff instead.
    let env = Env::mixed(&[("g1", "antigravity"), ("g2", "antigravity")]);
    env.script("g1.jsonl", &agy_ok("conv-1", "first answer"));
    let (code, _) = env.run(&[]);
    assert_eq!(code, 0);
    std::fs::remove_file(env.path("cfg/g1/.gemini/antigravity-cli/conversations/conv-1.db"))
        .unwrap();
    env.script("g1.2.jsonl", &agy_ok("conv-2", "fresh answer"));
    let (code, r) = env.run(&["--follow-up", "1"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "fresh answer");
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(!launches[1].contains("--conversation"), "{launches:?}");
}
