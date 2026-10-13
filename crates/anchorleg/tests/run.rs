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
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":4102444800,"rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.2,"resetsAt":4102444800}}}}"#,
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
                "codex" => ("fake-codex.py", "CODEX_HOME"),
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
            .env("ANCHORLEG_NOTIFY_LOG", self.path("notify.log"))
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

    /// The notices anchorleg sent (`ANCHORLEG_NOTIFY_LOG`), as "event: message".
    fn notices(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("notify.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                format!(
                    "{}: {}",
                    v["event"].as_str().unwrap(),
                    v["message"].as_str().unwrap()
                )
            })
            .collect()
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
    assert_eq!(
        env.notices(),
        ["switched: switching a → b", "done: #1 done: finished on b"]
    );
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
fn an_accounts_own_limit_stops_it_earlier() {
    let env = Env::new(&["a", "b"]);
    // a ends at 50% of its five-hour window.
    env.script(
        "a.jsonl",
        &ok("s1", "[anchorleg] Pausing here").replace("\"utilization\":0.2", "\"utilization\":0.5"),
    );
    env.script("b.jsonl", &ok("s1", "finished on b"));
    env.anchorleg()
        .args(["accounts", "set", "a", "--stop-at", "0.4"])
        .assert()
        .success();
    let list = env.anchorleg().args(["accounts", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains("stops at 40%"));
    let mut cmd = env.anchorleg();
    cmd.env("FAKE_MOD_STOP", "a");

    let (code, r) = env.run_with(cmd, &[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["switches"][0]["rule"], "quota_stop");
    let launches = env.launches();
    assert!(launches[0].ends_with("[stop_at=0.4]"), "{launches:?}");
    assert!(launches[1].ends_with("[stop_at=0.9]"), "{launches:?}");

    // a is past its own 40% now: the next task starts on b.
    env.script("b.jsonl", &ok("s2", "second"));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["accounts_used"], serde_json::json!(["b"]), "{r}");
}

#[test]
fn a_task_stops_once_its_token_budget_is_used() {
    let env = Env::new(&["a"]);
    // The first turn counts 10,100 tokens (90,000 cache reads count as a tenth) and isn't done.
    env.script(
        "a.jsonl",
        &ok("s1", "GOAL: NOT MET — still red").replace(
            r#""session_id":"s1"}"#,
            r#""session_id":"s1","usage":{"input_tokens":600,"output_tokens":300,"cache_creation_input_tokens":200,"cache_read_input_tokens":90000}}"#,
        ),
    );
    let (code, r) = env.run(&["--goal", "tests pass", "--budget", "10k"]);
    assert_eq!(code, 3, "{r}");
    assert_eq!(r["outcome"], "over_budget", "{r}");
    assert_eq!(
        r["budget"],
        serde_json::json!({ "tokens": 10000, "spent": 10100 }),
        "{r}"
    );
    assert_eq!(env.launches().len(), 1, "{:?}", env.launches());
    // Claude launches are told what's left, for anchorleg-mod's stop inside a turn.
    assert!(
        env.launches()[0].contains("[budget_left=10000]"),
        "{:?}",
        env.launches()
    );
}

#[test]
fn a_json_schema_brings_the_answer_back_as_structured_output() {
    let schema = r#"{"type":"object","properties":{"answer":{"type":"integer"}}}"#;
    // As anchorleg passes it on: the same JSON, keys sorted.
    let sent = serde_json::from_str::<Value>(schema).unwrap().to_string();
    let env = Env::new(&["a"]);
    env.script(
        "a.jsonl",
        &ok("s1", r#"{\"answer\":42}"#).replace(
            r#""session_id":"s1"}"#,
            r#""session_id":"s1","structured_output":{"answer":42}}"#,
        ),
    );
    let (code, r) = env.run(&["--json-schema", schema]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(
        r["structured_output"],
        serde_json::json!({ "answer": 42 }),
        "{r}"
    );
    assert!(
        env.launches()[0].contains(&format!("--json-schema {sent}")),
        "{:?}",
        env.launches()
    );

    // Codex answers with the JSON as its message.
    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.jsonl", &codex_ok("t1", r#"{\"answer\":7}"#));
    let path = env.path("schema.json");
    std::fs::write(&path, schema).unwrap();
    let (code, r) = env.run(&["--json-schema", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(
        r["structured_output"],
        serde_json::json!({ "answer": 7 }),
        "{r}"
    );
    assert!(
        env.launches()[0].contains(&format!("schema={sent}")),
        "{:?}",
        env.launches()
    );
}

#[test]
fn agys_question_tool_puts_questions_to_the_person() {
    use std::io::{BufRead, BufReader, Write};
    let env = Env::new(&["a"]);
    env.script("a.jsonl", &ok("s1", "done"));
    // Run 1 stays at work while its agent asks.
    let std_cmd = |args: &[&str]| {
        let mut c = std::process::Command::new(assert_cmd::cargo::cargo_bin("anchorleg"));
        c.args(args)
            .env("ANCHORLEG_CONFIG", env.path("config.toml"))
            .env("ANCHORLEG_HOME", env.path("home"))
            .env("ANCHORLEG_NOTIFY_LOG", env.path("notify.log"))
            .env("HOME", env.path("userhome"))
            .env("FAKE_DIR", env.path("fake"))
            .env("FAKE_LOG", env.path("fake.log"));
        c
    };
    let work = env.path("work");
    let mut run = std_cmd(&["run", "--json", "--cwd", work.to_str().unwrap(), "--", "go"])
        .env("FAKE_SLEEP", "30")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        let out = std_cmd(&["sessions", "--json"]).output().unwrap();
        if String::from_utf8_lossy(&out.stdout).contains("running") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // What agy starts in an account's own HOME: anchorleg's MCP server, inside run 1.
    let mut server = std_cmd(&["permission", "mcp-ask"])
        .env("ANCHORLEG_RUN_ID", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut lines = BufReader::new(server.stdout.take().unwrap()).lines();
    let mut call = |msg: Value| -> Value {
        writeln!(stdin, "{msg}").unwrap();
        serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
    };
    let init = call(
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18" } }),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "anchorleg");
    let tools = call(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    assert_eq!(tools["result"]["tools"][0]["name"], "ask_user");

    // The person answers from another terminal while the tool waits.
    let answerer = {
        let (config, home) = (env.path("config.toml"), env.path("home"));
        let cmd = move || {
            let mut c = Command::cargo_bin("anchorleg").unwrap();
            c.env("ANCHORLEG_CONFIG", &config)
                .env("ANCHORLEG_HOME", &home);
            c
        };
        std::thread::spawn(move || {
            for _ in 0..100 {
                let out = cmd()
                    .args(["permission", "list", "--json"])
                    .output()
                    .unwrap();
                let pending: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
                if let Some(id) = pending.get(0).and_then(|p| p["id"].as_i64()) {
                    cmd()
                        .args(["permission", "answer", &id.to_string(), "2"])
                        .assert()
                        .success();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            panic!("the question never showed up");
        })
    };
    let reply = call(
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "ask_user", "arguments": { "questions": [{
            "question": "Which database?", "header": "DB",
            "options": [{ "label": "SQLite" }, { "label": "Postgres" }] }] } } }),
    );
    answerer.join().unwrap();
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Postgres"), "{text}");
    drop(stdin);
    server.wait().unwrap();
    std_cmd(&["stop", "1"]).output().unwrap();
    run.wait().unwrap();
}

#[test]
fn a_session_has_a_name_its_follow_ups_keep() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "first"));
    env.script("a.2.jsonl", &ok("s1", "second"));
    assert_eq!(env.run(&["--name", "auth fix"]).0, 0);
    assert_eq!(env.run(&["--follow-up", "1"]).0, 0);
    let launches = env.launches();
    assert!(launches[0].contains("--name auth fix"), "{launches:?}");
    assert!(launches[1].contains("--name auth fix"), "{launches:?}");
    let out = env.anchorleg().args(["sessions"]).output().unwrap();
    let list = String::from_utf8_lossy(&out.stdout);
    assert_eq!(list.matches("[auth fix]").count(), 2, "{list}");
    env.anchorleg()
        .args(["rename", "2", "login flow"])
        .assert()
        .success()
        .stdout("#2: login flow\n");
    let out = env.anchorleg().args(["sessions"]).output().unwrap();
    let list = String::from_utf8_lossy(&out.stdout);
    assert!(
        list.contains("[login flow]") && list.contains("[auth fix]"),
        "{list}"
    );

    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.jsonl", &codex_ok("t1", "done"));
    assert_eq!(env.run(&["--name", "parser"]).0, 0);
    let names = std::fs::read_to_string(env.path("fake/names.log")).unwrap();
    assert_eq!(names, "t1 parser\n");
}

#[test]
fn another_model_answering_shows_in_the_conversation() {
    let env = Env::new(&["a"]);
    // Started on Haiku; a reply comes from Sonnet (e.g. plan mode or an overload fallback).
    let reply = r#"{"type":"assistant","session_id":"s1","parent_tool_use_id":null,"message":{"model":"claude-sonnet-5-5","content":[{"type":"text","text":"hi"}]}}"#;
    let sub = r#"{"type":"assistant","session_id":"s1","parent_tool_use_id":"tu1","message":{"model":"claude-opus-5-5","content":[]}}"#;
    let script = ok("s1", "done").replacen('\n', &format!("\n{reply}\n{sub}\n{reply}\n"), 1);
    env.script("a.jsonl", &script);
    assert_eq!(env.run(&[]).0, 0);
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert_eq!(
        log.matches("a answers with claude-sonnet-5-5 instead of claude-haiku-5-5")
            .count(),
        1,
        "{log}"
    );
    assert!(!log.contains("answers with claude-opus"), "{log}");
}

#[test]
fn background_commands_are_noted_when_their_launch_ends() {
    let env = Env::new(&["a"]);
    let bg = r#"{"type":"assistant","session_id":"s1","message":{"content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"npm run dev","run_in_background":true}}]}}"#;
    env.script(
        "a.jsonl",
        &ok("s1", "started the server").replacen('\n', &format!("\n{bg}\n"), 1),
    );
    assert_eq!(env.run(&[]).0, 0);
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(
        log.contains("background commands the agent started on a ended with it: npm run dev"),
        "{log}"
    );
    // The person's hooks show in the conversation.
    assert!(
        env.launches()[0].contains("--include-hook-events"),
        "{:?}",
        env.launches()
    );
}

#[test]
fn a_strict_limit_waits_instead_of_carrying_on() {
    let env = Env::new(&["a"]);
    // a ends at 50% of its five-hour window.
    env.script(
        "a.jsonl",
        &ok("s1", "[anchorleg] Pausing here").replace("\"utilization\":0.2", "\"utilization\":0.5"),
    );
    env.anchorleg()
        .args([
            "accounts",
            "set",
            "a",
            "--stop-at",
            "0.4",
            "--strict",
            "true",
        ])
        .assert()
        .success();
    let list = env.anchorleg().args(["accounts", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains("stops at 40% (strict)"));
    let mut cmd = env.anchorleg();
    cmd.env("FAKE_MOD_STOP", "a");

    let (code, r) = env.run_with(cmd, &["--no-wait"]);
    assert_eq!(code, 75, "{r}");
    assert_eq!(r["wait_until"], 4102444800_i64, "{r}");
    assert_eq!(env.launches().len(), 1, "{:?}", env.launches());
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
        .env("ANCHORLEG_NOTIFY_LOG", env.path("notify.log"))
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
        .env("ANCHORLEG_NOTIFY_LOG", env.path("notify.log"))
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

/// `codex app-server` messages for a turn that answers `text`.
fn codex_ok(thread: &str, text: &str) -> String {
    format!(
        "{}\n{}\n{}\n{}\n",
        format_args!(
            r#"{{"method":"turn/started","params":{{"threadId":"{thread}","turn":{{"id":"u1","status":"inProgress"}}}}}}"#
        ),
        format_args!(
            r#"{{"method":"item/completed","params":{{"threadId":"{thread}","item":{{"type":"agentMessage","id":"m1","text":"{text}"}}}}}}"#
        ),
        format_args!(
            r#"{{"method":"thread/tokenUsage/updated","params":{{"threadId":"{thread}","tokenUsage":{{"last":{{"inputTokens":1200,"cachedInputTokens":0,"outputTokens":10}}}}}}}}"#
        ),
        format_args!(
            r#"{{"method":"turn/completed","params":{{"threadId":"{thread}","turn":{{"id":"u1","status":"completed","error":null}}}}}}"#
        ),
    )
}

/// A turn that runs a command and then hits the usage limit.
fn codex_limit(thread: &str) -> String {
    format!(
        "{}\n{}\n",
        format_args!(
            r#"{{"method":"item/completed","params":{{"threadId":"{thread}","item":{{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc 'cargo test'","aggregatedOutput":"","exitCode":0,"status":"completed"}}}}}}"#
        ),
        format_args!(
            r#"{{"method":"turn/completed","params":{{"threadId":"{thread}","turn":{{"id":"u1","status":"failed","error":{{"message":"You've hit your usage limit. Upgrade or try again later.","codexErrorInfo":"usageLimitExceeded"}}}}}}}}"#
        ),
    )
}

/// Codex asking to run `command`, offering only accept or cancel (as seen on 0.153.4).
fn codex_asks(thread: &str, command: &str) -> String {
    format!(
        r#"{{"method":"item/commandExecution/requestApproval","id":0,"params":{{"kind":"command","threadId":"{thread}","turnId":"u1","itemId":"c9","startedAtMs":1,"reason":"needs network","command":"/bin/zsh -lc '{command}'","cwd":"/w","commandActions":[{{"type":"unknown","command":"{command}"}}],"availableDecisions":["accept","cancel"]}}}}"#
    ) + "\n"
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
            "c1 thread/start model=gpt-5.6-luna effort=low sandbox=workspace-write approval=on-request -- fix the tests"
        ]
    );
    // Codex may ask the person questions outside plan mode too (D36).
    assert_eq!(
        std::fs::read_to_string(env.path("fake/features.log")).unwrap(),
        "c1 features.default_mode_request_user_input=true\n"
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
        launches[1].starts_with("c2 thread/resume t1 ")
            && launches[1].ends_with("-- Continue from where you stopped."),
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
    // Codex doesn't read CLAUDE.md: what it says beyond importing AGENTS.md goes along.
    std::fs::write(
        env.path("work/CLAUDE.md"),
        "@AGENTS.md\n\nRun the linter before finishing.\n",
    )
    .unwrap();
    std::fs::write(env.path("work/AGENTS.md"), "Codex reads this itself.\n").unwrap();

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on codex");
    assert_eq!(r["accounts_used"], serde_json::json!(["a", "c"]));
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(launches[1].starts_with("c thread/start "), "{launches:?}");
    assert!(!launches[1].contains("resume"));
    assert!(launches[1].contains("You are taking over a task"));
    assert!(launches[1].contains("# Task handoff"));
    assert!(
        launches[1].contains("The project's CLAUDE.md")
            && launches[1].contains("Run the linter before finishing."),
        "{launches:?}"
    );
    assert!(!launches[1].contains("@AGENTS.md"), "{launches:?}");
    assert!(
        !launches[1].contains("Codex reads this itself"),
        "{launches:?}"
    );
}

#[test]
fn a_goal_keeps_the_session_going_until_it_is_met() {
    let env = Env::new(&["a"]);
    env.script(
        "a.1.jsonl",
        &ok("s1", "Ran the tests.\\nGOAL: NOT MET — 1 test fails"),
    );
    env.script("a.2.jsonl", &ok("s1", "Fixed it.\\nGOAL: MET — 14 passed"));
    let (code, r) = env.run(&["--goal", "cargo test passes"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["goal"]["status"], "met", "{r}");
    assert_eq!(r["goal"]["turns"], 2);
    assert_eq!(r["goal"]["reason"], "14 passed");
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    // Every turn is told the goal; the second continues the same session.
    assert!(
        launches[0].contains(
            "Goal (anchorleg keeps this session going until it holds): cargo test passes"
        )
    );
    assert!(
        launches[1].contains("The goal isn't met yet (1 test fails)"),
        "{launches:?}"
    );
    assert!(launches[1].contains("--resume s1"));
    assert!(launches[1].contains("GOAL: MET —"));
    let sessions = env
        .anchorleg()
        .args(["sessions", "--json"])
        .output()
        .unwrap();
    let list: Value = serde_json::from_slice(&sessions.stdout).unwrap();
    assert_eq!(list[0]["goal_status"], "met", "{list}");

    // Out of turns: the run fails, and a follow-up keeps working toward the goal.
    let env = Env::new(&["a"]);
    env.script("a.jsonl", &ok("s1", "GOAL: NOT MET — still red"));
    let (code, r) = env.run(&["--goal", "cargo test passes", "--goal-turns", "2"]);
    assert_ne!(code, 0, "{r}");
    assert_eq!(r["outcome"], "failed");
    assert_eq!(r["goal"]["status"], "unmet");
    assert_eq!(env.launches().len(), 2);
    let (_, r) = env.run(&["--follow-up", "1", "--goal-turns", "1"]);
    assert_eq!(r["goal"]["condition"], "cargo test passes", "{r}");

    // "Impossible" ends it at once.
    let env = Env::new(&["a"]);
    env.script(
        "a.jsonl",
        &ok("s1", "GOAL: IMPOSSIBLE — the API was removed"),
    );
    let (code, r) = env.run(&["--goal", "use the old API"]);
    assert_ne!(code, 0);
    assert_eq!(r["goal"]["status"], "impossible");
    assert_eq!(env.launches().len(), 1);
}

#[test]
fn a_review_reads_the_sessions_changes_on_another_account_and_can_send_fixes() {
    let env = Env::mixed(&[("a", "claude"), ("c", "codex")]);
    let work = env.path("work");
    git(&work, &["init", "-q"]);
    std::fs::write(work.join("calc.py"), "return a + b\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "init"]);
    env.script("a.1.jsonl", &ok("s1", "changed it"));
    assert_eq!(env.run(&["--account", "a"]).0, 0);
    // What the session "did".
    std::fs::write(work.join("calc.py"), "return a - b\n").unwrap();

    env.script(
        "c.jsonl",
        &codex_ok(
            "t1",
            "- [P1] add subtracts — calc.py:1  VERDICT: changes requested",
        ),
    );
    env.script("a.2.jsonl", &ok("s1", "fixed"));
    let out = env
        .anchorleg()
        .args(["review", "1", "--fix", "--json"])
        .output()
        .unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["outcome"], "done", "{r}");
    assert_eq!(r["accounts_used"], serde_json::json!(["c"]));
    let launches = env.launches();
    // On the other account, read-only, in Codex's review mode, with the diff.
    assert!(launches[1].starts_with("c "), "{launches:?}");
    assert!(launches[1].contains("sandbox=read-only"), "{launches:?}");
    assert!(launches[1].contains("[review] Review the changes anchorleg session #1 made"));
    assert!(launches[1].contains("+return a - b"), "{launches:?}");
    // --fix: the findings go back to the reviewed session.
    wait_launches(&env, 3);
    let launches = env.launches();
    assert!(launches[2].starts_with("a "), "{launches:?}");
    assert!(launches[2].contains("--resume s1"));
    assert!(launches[2].contains("A reviewer (c) looked at the changes"));
    let sessions = env
        .anchorleg()
        .args(["sessions", "--json"])
        .output()
        .unwrap();
    let list: Value = serde_json::from_slice(&sessions.stdout).unwrap();
    let review = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == 2)
        .unwrap();
    assert_eq!(review["review_of"], 1, "{review}");

    // On Claude, a review runs in plan mode; nothing changed → nothing to review.
    let env = Env::new(&["a", "b"]);
    let work = env.path("work");
    git(&work, &["init", "-q"]);
    std::fs::write(work.join("x"), "1\n").unwrap();
    env.script("a.jsonl", &ok("s1", "done"));
    env.script("b.jsonl", &ok("s2", "VERDICT: approve"));
    assert_eq!(env.run(&["--account", "a"]).0, 0);
    let out = env.anchorleg().args(["review", "1"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("changed nothing"));
    std::fs::write(work.join("x"), "2\n").unwrap();
    let out = env
        .anchorleg()
        .args(["review", "1", "--json"])
        .output()
        .unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "VERDICT: approve", "{r}");
    let launches = env.launches();
    assert!(launches[1].starts_with("b "), "{launches:?}");
    // Not plan mode (Claude would answer it with a bigger model); no editing tools instead.
    assert!(
        launches[1].contains("--permission-mode default"),
        "{launches:?}"
    );
    assert!(launches[1].contains("--disallowedTools Edit,Write,NotebookEdit"));
}

#[test]
fn a_goal_survives_a_switch_to_another_cli() {
    let env = Env::mixed(&[("a", "claude"), ("c", "codex")]);
    env.script("a.jsonl", &limit("s1", FAR));
    env.script(
        "c.jsonl",
        &codex_ok("t9", "GOAL: MET — tests pass on codex"),
    );
    let (code, r) = env.run(&["--goal", "cargo test passes"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["goal"]["status"], "met", "{r}");
    let launches = env.launches();
    assert!(
        launches[1].contains("## Done when cargo test passes"),
        "{launches:?}"
    );
    assert!(launches[1].contains("Goal (anchorleg keeps this session going"));
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

/// `claude -p /usage`'s JSON answer, as `fixtures/claude-2.1.296-usage-20261013/` has it.
fn claude_usage(session: u32, week: u32) -> String {
    format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"num_turns":0,"total_cost_usd":0,"local_command":"usage","result":"You are currently using your subscription to power your Claude Code usage\n\nCurrent session: {session}% used\nCurrent week (all models): {week}% used"}}"#
    )
}

#[test]
fn live_claude_quota_sees_use_from_outside_anchorleg() {
    let env = Env::new(&["a", "b"]);
    // a's week was used up in an interactive session anchorleg never saw.
    env.script("a.usage.json", &claude_usage(0, 100));
    env.script("b.usage.json", &claude_usage(10, 30));
    env.script("a.jsonl", &ok("s1", "ran on a"));
    env.script("b.jsonl", &ok("s2", "ran on b"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["accounts_used"], serde_json::json!(["b"]), "{r}");
    assert!(env.launches().iter().all(|l| l.starts_with("b ")));
    let asked = std::fs::read_to_string(env.path("fake/usage.log")).unwrap();
    assert_eq!(asked.lines().count(), 2, "{asked}");

    // b's run just reported its own quota: the next pick doesn't ask b again.
    env.script("a.usage.json", &claude_usage(0, 5));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["accounts_used"], serde_json::json!(["a"]), "{r}");
    let asked = std::fs::read_to_string(env.path("fake/usage.log")).unwrap();
    assert_eq!(
        asked.lines().collect::<Vec<_>>(),
        ["a", "b", "a"],
        "{asked}"
    );
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
        "g1 --dangerously-skip-permissions -p fix the tests --output-format stream-json --model gemini-3.8-flash-low --effort low"
    );
    // Each login has its own HOME, so anchorleg's approval hook is there (D26).
    let hooks = std::fs::read_to_string(env.path("cfg/g1/.gemini/config/hooks.json")).unwrap();
    assert!(hooks.contains("permission agy-hook"), "{hooks}");
    let mcp = std::fs::read_to_string(env.path("cfg/g1/.gemini/config/mcp_config.json")).unwrap();
    assert!(mcp.contains("mcp-ask"), "{mcp}");
    assert!(
        launches[1]
            .starts_with("g2 --dangerously-skip-permissions -p Continue from where you stopped.")
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
        launches[1].starts_with("g --dangerously-skip-permissions -p You are taking over a task"),
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

/// Start `anchorleg run` in the background with extra environment, for tests that answer
/// questions while it runs.
fn spawn_run(env: &Env, vars: &[(&str, &str)]) -> std::process::Child {
    spawn_run_with(env, &[], vars)
}

/// `anchorleg run` with `flags` added before the task.
fn spawn_run_with(env: &Env, flags: &[&str], vars: &[(&str, &str)]) -> std::process::Child {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("anchorleg"));
    cmd.args(["run", "--json"])
        .args(flags)
        .arg("--cwd")
        .arg(env.path("work"))
        .args(["--", "fix", "the", "tests"])
        .env("ANCHORLEG_CONFIG", env.path("config.toml"))
        .env("ANCHORLEG_HOME", env.path("home"))
        .env("ANCHORLEG_NOTIFY_LOG", env.path("notify.log"))
        .env("HOME", env.path("userhome"))
        .env("FAKE_DIR", env.path("fake"))
        .env("FAKE_LOG", env.path("fake.log"))
        .stdout(std::process::Stdio::piped());
    for (k, v) in vars {
        cmd.env(k, v);
    }
    cmd.spawn().unwrap()
}

/// Wait for the run's first question and return it.
fn first_question(env: &Env) -> Value {
    let started = std::time::Instant::now();
    loop {
        let out = env
            .anchorleg()
            .args(["permission", "list", "--json"])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        if v[0]["id"].is_i64() {
            return v[0].clone();
        }
        assert!(started.elapsed().as_secs() < 10, "nothing asked");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn agy_commands_wait_for_the_users_answer() {
    for (answer, decision) in [("yes", "allow"), ("no", "deny")] {
        let env = Env::mixed(&[("g", "antigravity")]);
        env.script("g.jsonl", &agy_ok("conv-1", "tests pass"));
        let child = spawn_run(&env, &[("FAKE_HOOK", "cargo test")]);
        let q = first_question(&env);
        assert_eq!(q["tool"], "Bash");
        assert_eq!(q["input"]["command"], "cargo test");
        env.anchorleg()
            .args(["permission", "answer", &q["id"].to_string(), answer])
            .assert()
            .success();
        let out = child.wait_with_output().unwrap();
        let r: Value = serde_json::from_slice(&out.stdout).unwrap();
        let hook: Value =
            serde_json::from_str(&std::fs::read_to_string(env.path("fake/hook.out")).unwrap())
                .unwrap();
        assert_eq!(hook["decision"], decision, "{hook}");
        let expected = if answer == "yes" {
            "tests pass"
        } else {
            "The command was refused."
        };
        assert_eq!(r["result"], expected, "{r}");
    }
}

#[test]
fn agy_edits_are_asked_about_in_ask_mode_and_refused_while_planning() {
    // `--mode ask`: an edit inside the task folder waits for the person.
    let env = Env::mixed(&[("g", "antigravity")]);
    env.script("g.jsonl", &agy_ok("conv-1", "edited"));
    let child = spawn_run_with(&env, &["--mode", "ask"], &[("FAKE_HOOK", "edit:a.rs")]);
    let q = first_question(&env);
    assert_eq!(q["tool"], "Write");
    assert!(
        q["input"]["file_path"].as_str().unwrap().ends_with("/a.rs"),
        "{q}"
    );
    env.anchorleg()
        .args(["permission", "answer", &q["id"].to_string(), "yes"])
        .assert()
        .success();
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "edited", "{r}");

    // `--plan`: the edit is refused with a note the agent reads; nobody is asked.
    let env = Env::mixed(&[("g", "antigravity")]);
    env.script("g.jsonl", &agy_ok("conv-1", "1. Change a.rs"));
    let mut child = spawn_run_with(&env, &["--plan"], &[("FAKE_HOOK", "edit:a.rs")]);
    // The plan's approval is the first (and only) question.
    let q = first_question(&env);
    let _ = child.kill();
    let _ = child.wait();
    assert_ne!(q["tool"], "Write", "{q}");
    let hook: Value =
        serde_json::from_str(&std::fs::read_to_string(env.path("fake/hook.out")).unwrap()).unwrap();
    assert_eq!(hook["decision"], "deny", "{hook}");
    assert!(
        hook["reason"]
            .as_str()
            .unwrap()
            .starts_with("Planning only"),
        "{hook}"
    );
}

#[test]
fn agy_on_the_persons_own_home_only_accepts_edits() {
    let env = Env::mixed(&[("g", "antigravity")]);
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake/fake-agy.sh");
    std::fs::write(
        env.path("config.toml"),
        format!(
            "[[account]]\nname = \"g\"\nvendor = \"antigravity\"\npriority = 1\nbin = \"{}\"\n",
            fake.display()
        ),
    )
    .unwrap();
    env.script("userhome.jsonl", &agy_ok("conv-1", "done"));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    let launches = env.launches();
    assert!(launches[0].contains("--mode accept-edits"), "{launches:?}");
    assert!(!launches[0].contains("--dangerously-skip-permissions"));
    assert!(!env.path("userhome/.gemini/config/hooks.json").exists());
    assert!(!env.path("userhome/.gemini/config/mcp_config.json").exists());

    // Planning uses agy's own plan mode there.
    let mut child = spawn_run_with(&env, &["--plan"], &[]);
    let _ = first_question(&env);
    let _ = child.kill();
    let _ = child.wait();
    let launches = env.launches();
    assert!(
        launches.last().unwrap().contains("--mode plan"),
        "{launches:?}"
    );
}

#[test]
fn codex_commands_wait_for_the_users_answer() {
    for answer in ["yes", "no"] {
        let env = Env::mixed(&[("c", "codex")]);
        env.script(
            "c.jsonl",
            &(codex_asks("t1", "curl -sI https://example.com") + &codex_ok("t1", "fetched it")),
        );
        env.script("c.followup.jsonl", &codex_ok("t1", "carried on without it"));
        let child = spawn_run(&env, &[]);
        let q = first_question(&env);
        assert_eq!(q["tool"], "Bash");
        assert_eq!(q["input"]["command"], "curl -sI https://example.com");
        assert_eq!(q["reason"], "needs network");
        env.anchorleg()
            .args(["permission", "answer", &q["id"].to_string(), answer])
            .assert()
            .success();
        let out = child.wait_with_output().unwrap();
        let r: Value = serde_json::from_slice(&out.stdout).unwrap();
        let answers = std::fs::read_to_string(env.path("fake/answers.log")).unwrap();
        let launches = env.launches();
        if answer == "yes" {
            assert!(answers.contains(r#""decision": "accept""#), "{answers}");
            assert_eq!(r["result"], "fetched it", "{r}");
            assert_eq!(launches.len(), 1, "{launches:?}");
        } else {
            // Codex offered no "decline": the turn is cancelled, and anchorleg continues the
            // thread with a note, like a Claude run told no.
            assert!(answers.contains(r#""decision": "cancel""#), "{answers}");
            assert_eq!(r["result"], "carried on without it", "{r}");
            assert!(
                launches[1]
                    .contains("I refused this, so it didn't run: `curl -sI https://example.com`."),
                "{launches:?}"
            );
        }
    }
}

#[test]
fn codex_quota_pushed_during_the_turn_is_recorded() {
    let env = Env::mixed(&[("c", "codex")]);
    let update = format!(
        r#"{{"method":"account/rateLimits/updated","params":{}}}"#,
        codex_limits(42, false)
    ) + "\n";
    env.script("c.jsonl", &(update + &codex_ok("t1", "done")));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status["accounts"][0]["windows"][0]["used"], 0.42,
        "{status}"
    );
}

#[test]
fn codex_stops_at_the_threshold_and_continues_on_a_roomier_login() {
    let env = Env::mixed(&[("c1", "codex"), ("c2", "codex")]);
    let update = format!(
        r#"{{"method":"account/rateLimits/updated","params":{}}}"#,
        codex_limits(95, false)
    ) + "\n";
    env.script(
        "c1.jsonl",
        &(codex_ok("t1", "").lines().next().unwrap().to_owned()
            + "\n"
            + &update
            + &codex_ok("t1", "finished on c1 anyway")),
    );
    env.script("c2.limits.json", &codex_limits(20, false));
    env.script("c2.jsonl", &codex_ok("t1", "finished on c2"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on c2");
    assert_eq!(r["switches"][0]["rule"], "quota_stop", "{r}");
    let interrupts = std::fs::read_to_string(env.path("fake/interrupts.log")).unwrap();
    assert!(interrupts.contains(r#""turnId": "u1""#), "{interrupts}");
    let launches = env.launches();
    assert!(
        launches[1].starts_with("c2 thread/resume t1 "),
        "{launches:?}"
    );
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(
        log.contains("c1 paused at a clean point (five_hour quota at 95%)"),
        "{log}"
    );
    assert!(env.path("work/.handoff/TASK.md").is_file());

    // `--stop-at 1` turns it off: the turn runs to the end on c1.
    let env = Env::mixed(&[("c1", "codex"), ("c2", "codex")]);
    env.script(
        "c1.jsonl",
        &(update.clone() + &codex_ok("t1", "finished on c1")),
    );
    let (code, r) = env.run(&["--stop-at", "1"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on c1");
}

#[test]
fn policies_pick_among_accounts_with_room() {
    // c1 is first by priority but 70% used; c2 is at 20%.
    let env = Env::mixed(&[("c1", "codex"), ("c2", "codex")]);
    env.script("c1.limits.json", &codex_limits(70, false));
    env.script("c2.limits.json", &codex_limits(20, false));
    env.script("c1.jsonl", &codex_ok("t1", "ran on c1"));
    env.script("c2.jsonl", &codex_ok("t2", "ran on c2"));

    let (_, r) = env.run(&[]);
    assert_eq!(r["result"], "ran on c1", "drain: priority order");
    let (_, r) = env.run(&["--policy", "balance"]);
    assert_eq!(r["result"], "ran on c2", "balance: least used");

    // The default policy is saved in the config and shown by `status`.
    env.anchorleg()
        .args(["policy", "balance"])
        .assert()
        .success();
    let (_, r) = env.run(&[]);
    assert_eq!(r["result"], "ran on c2");
    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["policy"], "balance");

    // --prefer and --account win over the policy; unknown names are refused.
    let (_, r) = env.run(&["--prefer", "c1"]);
    assert_eq!(r["result"], "ran on c1");
    let (_, r) = env.run(&["--account", "c1"]);
    assert_eq!(r["result"], "ran on c1");
    env.anchorleg()
        .args(["run", "--account", "nope", "--", "x"])
        .assert()
        .failure();

    // A pinned account goes first too; a weekly reserve keeps an account back.
    env.anchorleg()
        .args(["accounts", "set", "c1", "--pinned", "true"])
        .assert()
        .success();
    let (_, r) = env.run(&[]);
    assert_eq!(r["result"], "ran on c1");
    env.anchorleg()
        .args([
            "accounts",
            "set",
            "c1",
            "--pinned",
            "false",
            "--reserve-7d",
            "0.95",
        ])
        .assert()
        .success();
    let list = env.anchorleg().args(["accounts", "list"]).output().unwrap();
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("weekly reserve 95%"),
        "{}",
        String::from_utf8_lossy(&list.stdout)
    );
    env.anchorleg().args(["policy", "drain"]).assert().success();
    // c1's weekly is at 10%, inside a 95% reserve (it may use only 5%): c2 runs.
    let (_, r) = env.run(&[]);
    assert_eq!(r["result"], "ran on c2");
}

/// A rate limit event with extra-usage fields (Claude's "usage credits").
fn overage(status: &str, overage: &str, using: bool) -> String {
    format!(
        r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","resetsAt":{FAR},"rateLimitType":"five_hour","overageStatus":"{overage}","isUsingOverage":{using}}}}}"#
    )
}

fn ok_with_cost(sid: &str, text: &str, cost: f64) -> String {
    format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"result":"{text}","session_id":"{sid}","total_cost_usd":{cost},"modelUsage":{{"claude-haiku-5-5":{{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":300,"cacheCreationInputTokens":40,"costUSD":{cost}}}}}}}"#
    )
}

#[test]
fn paying_from_credits_without_permission_stops_and_switches() {
    let env = Env::new(&["a", "b"]);
    // a's plan is spent and the CLI carries on paying from extra usage: by default anchorleg
    // never spends credits, so it stops a and moves on.
    env.script(
        "a.jsonl",
        &format!(
            "{}\n{}\n{}\n",
            init("s1"),
            overage("allowed", "allowed", true),
            ok_with_cost("s1", "finished on a", 0.5)
        ),
    );
    env.script("b.jsonl", &ok("s1", "finished on b"));

    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on b");
    assert_eq!(r["accounts_used"], serde_json::json!(["a", "b"]));

    let status = env.anchorleg().args(["status", "--json"]).output().unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(status["accounts"][0]["blocked_until"].is_i64(), "{status}");
    assert_eq!(status["accounts"][0]["credits"]["state"], "in_use");
    assert_eq!(status["accounts"][0]["credits_use"], "never");
}

#[test]
fn last_resort_credits_carry_on_when_every_account_is_out() {
    let env = Env::new(&["a", "b"]);
    env.anchorleg()
        .args(["accounts", "set", "a", "--credits", "last-resort"])
        .assert()
        .success();
    // a runs out with extra usage available; anchorleg tries b first, which is out too, then
    // comes back to a and lets it pay.
    env.script(
        "a.1.jsonl",
        &format!(
            "{}\n{}\n{}\n",
            init("s1"),
            overage("rejected", "allowed", false),
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Claude AI usage limit reached","session_id":"s1"}"#
        ),
    );
    env.script("b.jsonl", &limit("s1", FAR));
    env.script(
        "a.2.jsonl",
        &format!(
            "{}\n{}\n{}\n",
            init("s1"),
            overage("rejected", "allowed", true),
            ok_with_cost("s1", "finished on credits", 0.25)
        ),
    );

    let (code, r) = env.run(&["--no-wait"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "finished on credits");
    assert_eq!(r["accounts_used"], serde_json::json!(["a", "b", "a"]));

    let usage = env
        .anchorleg()
        .args(["usage", "--json", "--since", "all"])
        .output()
        .unwrap();
    let usage: Value = serde_json::from_slice(&usage.stdout).unwrap();
    let a = &usage["totals"][0];
    assert_eq!(a["account"], "a");
    assert_eq!(a["model"], "claude-haiku-5-5");
    assert_eq!(a["cost_usd"], 0.25);
    assert_eq!(a["credits_cost_usd"], 0.25);
    assert_eq!(a["cache_read_tokens"], 300);

    // The text dashboard renders too.
    let text = env
        .anchorleg()
        .args(["usage", "--since", "all"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("claude-haiku-5-5"), "{text}");
    assert!(text.contains("ran on paid credits: $0.25"), "{text}");
}

#[test]
fn codex_questions_are_answered_through_anchorleg() {
    let env = Env::mixed(&[("c", "codex")]);
    let ask = r#"{"method":"item/tool/requestUserInput","id":0,"params":{"threadId":"t1","turnId":"u1","itemId":"q9","isBlocking":true,"questions":[{"id":"runner","header":"Runner","question":"Which test runner?","options":[{"label":"Vitest","description":"fast"},{"label":"Jest","description":"current"}]},{"id":"token","header":"Token","question":"npm token?","isSecret":true}]}}"#;
    env.script(
        "c.jsonl",
        &(ask.to_owned() + "\n" + &codex_ok("t1", "migrated")),
    );
    let child = spawn_run(&env, &[]);
    let q = first_question(&env);
    assert_eq!(q["tool"], "AskUserQuestion");
    assert_eq!(q["input"]["questions"][0]["options"][1]["label"], "Jest");
    assert_eq!(q["input"]["questions"][1]["secret"], true);

    // A secret is never passed: anchorleg refuses to take it.
    env.anchorleg()
        .args([
            "permission",
            "answer",
            &q["id"].to_string(),
            "1",
            "npm_abc123",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("doesn't pass secrets"));
    env.anchorleg()
        .args(["permission", "answer", &q["id"].to_string(), "1", "-"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Runner → Vitest; Token → (secret, not passed)",
        ));
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "migrated", "{r}");

    let answers = std::fs::read_to_string(env.path("fake/answers.log")).unwrap();
    let reply: Value = serde_json::from_str(answers.lines().next().unwrap()).unwrap();
    assert_eq!(
        reply["result"],
        serde_json::json!({ "answers": { "runner": { "answers": ["Vitest"] } } })
    );
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(log.contains("the agent asks: 2 questions"), "{log}");
    assert!(log.contains("Runner → Vitest"), "{log}");
}

#[test]
fn codex_mcp_prompts_and_sandbox_requests_reach_the_person() {
    let env = Env::mixed(&[("c", "codex")]);
    let access = r#"{"method":"item/permissions/requestApproval","id":0,"params":{"threadId":"t1","turnId":"u1","itemId":"p1","cwd":"/w","startedAtMs":1,"reason":"fetch deps","permissions":{"network":{"enabled":true}}}}"#;
    let form = r#"{"method":"mcpServer/elicitation/request","id":1,"params":{"serverName":"jira","threadId":"t1","turnId":"u1","mode":"form","message":"Which project?","requestedSchema":{"type":"object","properties":{"project":{"type":"string","enum":["ENG","OPS"]}}}}}"#;
    env.script(
        "c.jsonl",
        &format!("{access}\n{form}\n{}", codex_ok("t1", "filed")),
    );
    let child = spawn_run(&env, &[]);

    let p = first_question(&env);
    assert_eq!(p["tool"], "Permissions");
    let list = env
        .anchorleg()
        .args(["permission", "list"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("Permissions  network"),
        "{}",
        String::from_utf8_lossy(&list.stdout)
    );
    env.anchorleg()
        .args(["permission", "answer", &p["id"].to_string(), "yes"])
        .assert()
        .success();

    let started = std::time::Instant::now();
    let q = loop {
        let q = first_question(&env);
        if q["id"] != p["id"] {
            break q;
        }
        assert!(started.elapsed().as_secs() < 10);
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert_eq!(q["tool"], "AskUserQuestion");
    assert_eq!(q["input"]["questions"][0]["header"], "jira · project");
    env.anchorleg()
        .args(["permission", "answer", &q["id"].to_string(), "2"])
        .assert()
        .success();

    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "filed", "{r}");
    let answers: Vec<Value> = std::fs::read_to_string(env.path("fake/answers.log"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        answers[0]["result"],
        serde_json::json!({ "permissions": { "network": { "enabled": true } }, "scope": "turn" })
    );
    assert_eq!(
        answers[1]["result"],
        serde_json::json!({ "action": "accept", "content": { "project": "OPS" } })
    );
}

/// The person's answer to the question waiting at the moment, once a new one shows up.
fn answer_next_question(env: &Env, after: &Value, answer: &str) -> Value {
    let started = std::time::Instant::now();
    let q = loop {
        let q = first_question(env);
        if &q["id"] != after {
            break q;
        }
        assert!(started.elapsed().as_secs() < 10, "no new question");
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    env.anchorleg()
        .args(["permission", "answer", &q["id"].to_string(), answer])
        .assert()
        .success();
    q["id"].clone()
}

#[test]
fn a_plan_can_be_edited_before_it_is_approved() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "1. add docs/page.md"));
    env.script("a.2.jsonl", &ok("s1", "guide added"));
    let child = spawn_run_with(&env, &["--plan"], &[]);
    let q = first_question(&env);
    assert_eq!(q["input"]["plan"], "1. add docs/page.md", "{q}");
    // `edit` opens the plan in $EDITOR; here the "editor" changes it in place.
    env.anchorleg()
        .env("VISUAL", "perl -pi -e s/page/guide/")
        .args(["permission", "answer", &q["id"].to_string(), "edit"])
        .assert()
        .success();
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "guide added", "{r}");
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(
        launches[1].contains("The plan is approved, with my edits")
            && launches[1].contains("1. add docs/guide.md")
            && !launches[1].contains("--permission-mode plan"),
        "{launches:?}"
    );
}

#[test]
fn a_plan_is_approved_before_it_runs() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "1. add docs/page.md"));
    env.script("a.2.jsonl", &ok("s1", "1. add docs/page.md in Markdown"));
    env.script("a.3.jsonl", &ok("s1", "page added"));
    env.anchorleg()
        .args([
            "settings",
            "claude",
            "--args",
            "--permission-mode acceptEdits",
        ])
        .assert()
        .success();
    let child = spawn_run_with(&env, &["--plan"], &[]);

    // Their words send the plan back; then they approve it.
    let first = answer_next_question(&env, &Value::Null, "use Markdown");
    answer_next_question(&env, &first, "1");
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "page added", "{r}");

    let launches = env.launches();
    assert_eq!(launches.len(), 3, "{launches:?}");
    // Planning: plan mode replaces the person's own mode, and the agent is told how to hand
    // the plan over.
    assert!(
        launches[0].contains("--permission-mode plan"),
        "{launches:?}"
    );
    assert!(!launches[0].contains("acceptEdits"), "{launches:?}");
    assert!(
        launches[0].contains("anchorleg will ask me to approve it"),
        "{launches:?}"
    );
    assert!(
        launches[1].contains("Change the plan: use Markdown")
            && launches[1].contains("--resume s1"),
        "{launches:?}"
    );
    assert!(
        launches[1].contains("--permission-mode plan"),
        "{launches:?}"
    );
    // Approved: the same session, back in the person's own mode.
    assert!(launches[2].contains("The plan is approved"), "{launches:?}");
    assert!(
        launches[2].contains("--permission-mode acceptEdits"),
        "{launches:?}"
    );
    assert!(
        !launches[2].contains("--permission-mode plan"),
        "{launches:?}"
    );
    assert_eq!(
        env.notices(),
        [
            "waiting: #1 plan ready: approve it?",
            "waiting: #1 plan ready: approve it?",
            "done: #1 done: page added"
        ]
    );
}

#[test]
fn a_codex_plan_uses_plan_mode_until_approved() {
    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.1.jsonl", &codex_ok("t1", "1. add docs/page.md"));
    env.script("c.2.jsonl", &codex_ok("t1", "page added"));
    let child = spawn_run_with(&env, &["--plan"], &[]);
    answer_next_question(&env, &Value::Null, "1");
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "page added", "{r}");
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    // Planning: Codex's plan mode (its model taken from the thread), nothing writable.
    assert!(
        launches[0].contains("sandbox=read-only")
            && launches[0].contains("[mode: plan model=gpt-default experimental=True]"),
        "{launches:?}"
    );
    assert!(
        !launches[0].contains("anchorleg will ask me"),
        "{launches:?}"
    );
    assert!(
        launches[1].contains("thread/resume t1")
            && launches[1].contains("sandbox=workspace-write")
            && launches[1].contains("The plan is approved")
            && !launches[1].contains("[mode:"),
        "{launches:?}"
    );
}

#[test]
fn a_session_keeps_its_mode() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "first"));
    env.script("a.2.jsonl", &ok("s1", "second"));
    env.anchorleg()
        .args([
            "settings",
            "claude",
            "--args",
            "--permission-mode default --verbose",
        ])
        .assert()
        .success();
    let (code, r) = env.run(&["--mode", "edits"]);
    assert_eq!(code, 0, "{r}");
    let (code, r) = env.run(&["--follow-up", "1"]);
    assert_eq!(code, 0, "{r}");
    let launches = env.launches();
    for l in &launches {
        assert!(l.contains("--permission-mode acceptEdits"), "{launches:?}");
        assert!(!l.contains("--permission-mode default"), "{launches:?}");
    }
}

#[test]
fn auto_mode_lets_the_clis_own_reviewer_decide() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "first"));
    env.script("a.2.jsonl", &ok("s1", "second"));
    let (code, r) = env.run(&["--mode", "auto"]);
    assert_eq!(code, 0, "{r}");
    let (code, r) = env.run(&["--follow-up", "1"]);
    assert_eq!(code, 0, "{r}");
    let launches = env.launches();
    for l in &launches {
        assert!(l.contains("--permission-mode auto"), "{launches:?}");
        // The mod leaves Claude's asks to its classifier.
        assert!(l.contains("[permission_mode=auto]"), "{launches:?}");
    }

    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.1.jsonl", &codex_ok("t1", "first"));
    env.script("c.2.jsonl", &codex_ok("t2", "planned"));
    let (code, r) = env.run(&["--mode", "auto"]);
    assert_eq!(code, 0, "{r}");
    let launches = env.launches();
    assert!(
        launches[0].contains("approval=on-request reviewer=auto_review --"),
        "{launches:?}"
    );
}

#[test]
fn a_fork_starts_a_new_session_from_the_old_one() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "first"));
    env.script("a.2.jsonl", &ok("s2", "the other way"));
    assert_eq!(env.run(&[]).0, 0);
    let (code, r) = env.run(&["--fork", "1"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["session_id"], "s2");
    let launches = env.launches();
    assert!(
        launches[1].contains("--resume s1 --fork-session"),
        "{launches:?}"
    );

    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.1.jsonl", &codex_ok("t1", "first"));
    env.script("c.2.jsonl", &codex_ok("t1-fork", "the other way"));
    assert_eq!(env.run(&[]).0, 0);
    let (code, r) = env.run(&["--fork", "1"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["session_id"], "t1-fork", "{r}");
    let launches = env.launches();
    assert!(launches[1].contains("thread/fork t1"), "{launches:?}");
}

#[test]
fn a_task_can_run_in_its_own_worktree() {
    let env = Env::new(&["a"]);
    env.script("a.jsonl", &ok("s1", "done"));
    let work = env.path("work");
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&work)
                .args(&args)
                .status()
                .unwrap()
                .success()
        );
    }
    let (code, r) = env.run(&["--worktree", "demo"]);
    assert_eq!(code, 0, "{r}");
    let out = env
        .anchorleg()
        .args(["sessions", "--json"])
        .output()
        .unwrap();
    let runs: Value = serde_json::from_slice(&out.stdout).unwrap();
    let cwd = runs[0]["cwd"].as_str().unwrap();
    assert!(cwd.ends_with(".anchorleg/worktrees/demo"), "{cwd}");
    let branch = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&branch.stdout).trim(),
        "anchorleg/demo"
    );
}

#[test]
fn a_follow_up_stays_on_the_sessions_account() {
    // `a` (Claude) comes first by priority, but the session lives on Codex.
    let env = Env::mixed(&[("a", "claude"), ("c", "codex")]);
    env.script("c.1.jsonl", &codex_ok("t1", "first"));
    env.script("c.2.jsonl", &codex_ok("t1", "second"));
    env.script("a.jsonl", &ok("s9", "wrong account"));
    assert_eq!(env.run(&["--account", "c"]).0, 0);
    let (code, r) = env.run(&["--follow-up", "1"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "second", "{r}");
    let launches = env.launches();
    assert!(
        launches[1].starts_with("c ") && launches[1].contains("thread/resume t1"),
        "{launches:?}"
    );
}

#[test]
fn an_unanswered_question_lets_the_agent_go_on_after_the_wait() {
    let env = Env::mixed(&[("c", "codex")]);
    let ask = r#"{"method":"item/tool/requestUserInput","id":0,"params":{"threadId":"t1","turnId":"u1","itemId":"q9","isBlocking":true,"questions":[{"id":"runner","header":"Runner","question":"Which test runner?","options":[{"label":"Vitest","description":"fast"}]}]}}"#;
    env.script(
        "c.jsonl",
        &(ask.to_owned() + "\n" + &codex_ok("t1", "went with Vitest")),
    );
    let started = std::time::Instant::now();
    let (code, r) = env.run(&["--answer-wait", "10s"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["result"], "went with Vitest");
    assert!(started.elapsed().as_secs() >= 10, "{:?}", started.elapsed());
    let answers = std::fs::read_to_string(env.path("fake/answers.log")).unwrap();
    let reply: Value = serde_json::from_str(answers.lines().next().unwrap()).unwrap();
    assert_eq!(reply["result"], serde_json::json!({ "answers": {} }));
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(log.contains("nobody answered"), "{log}");
}

/// Wait until the fakes have seen `n` launches.
fn wait_launches(env: &Env, n: usize) {
    let started = std::time::Instant::now();
    while env.launches().len() < n {
        assert!(
            started.elapsed().as_secs() < 10,
            "only {:?}",
            env.launches()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn a_message_for_a_working_claude_session_continues_it_when_the_turn_ends() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "tests migrated"));
    env.script("a.2.jsonl", &ok("s1", "docs updated too"));
    let child = spawn_run(&env, &[("FAKE_SLEEP", "1")]);
    wait_launches(&env, 1);
    env.anchorleg()
        .args(["send", "1", "--", "also", "update", "the", "docs"])
        .assert()
        .success();
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "docs updated too", "{r}");
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(
        launches[1].contains("-p also update the docs"),
        "{launches:?}"
    );
    assert!(launches[1].contains("--resume s1"), "{launches:?}");
    // Delivered once, and shown as the person's message.
    let log = std::fs::read_to_string(env.path("home/runs/run-1.jsonl")).unwrap();
    assert!(
        log.contains(r#""text":"also update the docs","type":"anchorleg_user""#),
        "{log}"
    );
    env.anchorleg()
        .args(["send", "1", "--", "late"])
        .assert()
        .failure();
}

fn codex_waiting(thread: &str, text: &str) -> String {
    format!(
        "{}\n{}\n{}",
        format_args!(
            r#"{{"method":"turn/started","params":{{"threadId":"{thread}","turn":{{"id":"u1","status":"inProgress"}}}}}}"#
        ),
        r#"{"fake":"wait_steer"}"#,
        codex_ok(thread, text)
    )
}

#[test]
fn a_message_for_a_working_codex_turn_steers_it() {
    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.jsonl", &codex_waiting("t1", "used vitest"));
    let child = spawn_run(&env, &[]);
    wait_launches(&env, 1);
    env.anchorleg()
        .args(["send", "1", "--", "use", "vitest"])
        .assert()
        .success();
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "used vitest", "{r}");
    let steers = std::fs::read_to_string(env.path("fake/steers.log")).unwrap();
    let steer: Value = serde_json::from_str(steers.lines().next().unwrap()).unwrap();
    assert_eq!(steer["params"]["expectedTurnId"], "u1");
    assert_eq!(steer["params"]["input"][0]["text"], "use vitest");
    // Taken into the turn: no second launch for it.
    assert_eq!(env.launches().len(), 1, "{:?}", env.launches());
}

#[test]
fn interrupt_and_send_stops_the_codex_turn_and_continues_with_the_message() {
    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.1.jsonl", &codex_waiting("t1", "never seen"));
    env.script("c.2.jsonl", &codex_ok("t1", "moved to src/"));
    let child = spawn_run(&env, &[]);
    wait_launches(&env, 1);
    env.anchorleg()
        .args(["send", "1", "--interrupt", "--", "stop,", "use", "src/"])
        .assert()
        .success();
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "moved to src/", "{r}");
    assert!(env.path("fake/interrupts.log").is_file());
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(launches[1].contains("thread/resume t1"), "{launches:?}");
    assert!(launches[1].ends_with("-- stop, use src/"), "{launches:?}");
}

#[test]
fn compact_on_codex_compacts_the_thread() {
    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.jsonl", &codex_ok("t1", "done"));
    let (code, r) = env.run(&["--account", "c"]);
    assert_eq!(code, 0, "{r}");
    env.anchorleg()
        .args(["run", "--json", "--follow-up", "1", "--", "/compact"])
        .assert()
        .success();
    let launches = env.launches();
    assert!(
        launches
            .last()
            .unwrap()
            .ends_with("thread/compact/start t1"),
        "{launches:?}"
    );
    let log = std::fs::read_to_string(env.path("home/runs/run-2.jsonl")).unwrap();
    assert!(log.contains(r#""type":"context_compaction""#), "{log}");
}

#[test]
fn messages_for_a_working_agy_session_wait_or_interrupt() {
    // A plain message waits for the turn to end, then continues the same conversation.
    let env = Env::mixed(&[("g", "antigravity")]);
    env.script("g.1.jsonl", &agy_ok("conv-1", "first done"));
    env.script("g.2.jsonl", &agy_ok("conv-1", "docs updated"));
    let child = spawn_run(&env, &[("FAKE_SLEEP", "2")]);
    wait_launches(&env, 1);
    env.anchorleg()
        .args(["send", "1", "--", "also", "update", "the", "docs"])
        .assert()
        .success();
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "docs updated", "{r}");
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(
        launches[1].contains("-p also update the docs"),
        "{launches:?}"
    );
    assert!(
        launches[1].contains("--conversation conv-1"),
        "{launches:?}"
    );

    // "Interrupt" stops agy now and continues with the message.
    let env = Env::mixed(&[("g", "antigravity")]);
    env.script("g.1.jsonl", &agy_ok("conv-1", "never seen"));
    env.script("g.2.jsonl", &agy_ok("conv-1", "moved to src/"));
    let start = std::time::Instant::now();
    let child = spawn_run(&env, &[("FAKE_SLEEP", "30")]);
    wait_launches(&env, 1);
    env.anchorleg()
        .args(["send", "1", "--interrupt", "--", "stop,", "use", "src/"])
        .assert()
        .success();
    let out = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "moved to src/", "{r}");
    assert!(start.elapsed() < std::time::Duration::from_secs(20));
    let launches = env.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(launches[1].contains("-p stop, use src/"), "{launches:?}");
    assert!(
        launches[1].contains("--conversation conv-1"),
        "{launches:?}"
    );
}

#[test]
fn attached_files_are_copied_into_the_task_and_named_in_the_prompt() {
    let env = Env::mixed(&[("a", "claude"), ("c", "codex")]);
    let shot = env.path("Screen Shot.png");
    std::fs::write(&shot, b"\x89PNG fake").unwrap();
    let log = env.path("build.log");
    std::fs::write(&log, "error: boom").unwrap();

    // Claude: the prompt names the copies, which sit in the task folder, ignored by git.
    env.script("a.jsonl", &ok("s1", "looked"));
    let (code, r) = env.run(&[
        "--account",
        "a",
        "--attach",
        shot.to_str().unwrap(),
        "--attach",
        log.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{r}");
    let launch = env.launches()[0].clone();
    assert!(
        launch.contains("Attached (read them; paths are relative to the task folder):"),
        "{launch}"
    );
    let copies: Vec<_> = std::fs::read_dir(env.path("work/.anchorleg/attachments"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(copies.len(), 2, "{copies:?}");
    assert!(copies.iter().any(|c| c.ends_with("-Screen_Shot.png")));
    assert!(launch.contains(".anchorleg/attachments/"), "{launch}");
    assert!(env.path("work/.anchorleg/.gitignore").is_file());

    // Codex: images also go in as image input.
    env.script("c.jsonl", &codex_ok("t1", "saw it"));
    let (code, r) = env.run(&["--account", "c", "--attach", shot.to_str().unwrap()]);
    assert_eq!(code, 0, "{r}");
    let launch = env.launches().last().unwrap().clone();
    assert!(launch.contains("[images: "), "{launch}");
    assert!(launch.ends_with("-Screen_Shot.png]"), "{launch}");
}

#[test]
fn lasting_rules_reach_claude_and_answer_for_the_other_clis() {
    let env = Env::new(&["a"]);
    env.anchorleg()
        .args(["rules", "--add", "Bash(cargo test *)"])
        .assert()
        .success();
    env.anchorleg()
        .args(["rules", "--add", "Write(*.rs)"])
        .assert()
        .success();
    env.script("a.jsonl", &ok("s1", "done"));
    let (code, r) = env.run(&[]);
    assert_eq!(code, 0, "{r}");
    let launches = env.launches();
    assert!(
        launches[0].contains("--allowedTools Bash(cargo test *),Write(*.rs)"),
        "{launches:?}"
    );

    // Antigravity: the hook's question is answered by the rule, nobody is asked.
    let env = Env::mixed(&[("g", "antigravity")]);
    env.anchorleg()
        .args(["rules", "--add", "Write(*.rs)"])
        .assert()
        .success();
    env.script("g.jsonl", &agy_ok("conv-1", "edited"));
    let mut child = spawn_run_with(&env, &["--mode", "ask"], &[("FAKE_HOOK", "edit:a.rs")]);
    // Don't hang the suite if the rule doesn't answer: the question would wait for a person.
    for _ in 0..100 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("the rule didn't answer: {}", first_question(&env));
    }
    let r: Value = serde_json::from_reader(child.stdout.take().unwrap()).unwrap();
    assert_eq!(r["result"], "edited", "{r}");
    let hook: Value =
        serde_json::from_str(&std::fs::read_to_string(env.path("fake/hook.out")).unwrap()).unwrap();
    assert_eq!(hook["decision"], "allow", "{hook}");
}

#[test]
fn a_side_question_is_answered_by_a_read_only_fork() {
    let env = Env::new(&["a"]);
    env.script("a.1.jsonl", &ok("s1", "working on it"));
    env.script("a.2.jsonl", &ok("s2", "it uses SQLite"));
    assert_eq!(env.run(&[]).0, 0);
    let out = env
        .anchorleg()
        .args(["btw", "1", "--json", "--", "which", "database?"])
        .output()
        .unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "it uses SQLite", "{r}");
    assert_eq!(r["session_id"], "s2", "{r}");
    let launches = env.launches();
    let side = &launches[1];
    assert!(side.contains("--resume s1 --fork-session"), "{side}");
    assert!(
        side.contains("--disallowedTools Edit,Write,NotebookEdit"),
        "{side}"
    );
    assert!(
        side.contains("A side question") && side.contains("which database?"),
        "{side}"
    );

    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.1.jsonl", &codex_ok("t1", "working on it"));
    env.script("c.2.jsonl", &codex_ok("t1-fork", "SQLite"));
    assert_eq!(env.run(&[]).0, 0);
    let out = env
        .anchorleg()
        .args(["btw", "1", "--json", "--", "which", "database?"])
        .output()
        .unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "SQLite", "{r}");
    let launches = env.launches();
    assert!(
        launches[1].contains("thread/fork t1") && launches[1].contains("sandbox=read-only"),
        "{launches:?}"
    );
}

#[test]
fn rewind_goes_back_to_before_a_run_in_the_conversation_and_the_files() {
    let env = Env::new(&["a"]);
    let work = env.path("work");
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&work)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "start"]);
    let assistant = |sid: &str, uuid: &str| {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","session_id":"{sid}","parent_tool_use_id":null,"message":{{"role":"assistant","content":[{{"type":"text","text":"ok"}}]}}}}"#
        )
    };
    env.script(
        "a.1.jsonl",
        &format!("{}\n{}", assistant("s1", "m1"), ok("s1", "first")),
    );
    env.script(
        "a.2.jsonl",
        &format!("{}\n{}", assistant("s1", "m2"), ok("s1", "second")),
    );
    env.script("a.3.jsonl", &ok("s2", "the other way"));
    assert_eq!(env.run(&[]).0, 0);
    std::fs::write(work.join("a.txt"), "two\n").unwrap();
    assert_eq!(env.run(&["--follow-up", "1"]).0, 0);
    std::fs::write(work.join("a.txt"), "three\n").unwrap();
    std::fs::write(work.join("new.txt"), "made by #2\n").unwrap();

    let out = env
        .anchorleg()
        .args(["rewind", "2", "--json", "--", "try", "another", "way"])
        .output()
        .unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "the other way", "{r}");
    let launches = env.launches();
    assert!(
        launches[2].contains("--resume s1 --resume-session-at m1 --fork-session"),
        "{launches:?}"
    );
    // The files as #2 found them; a backup of how they were is kept.
    assert_eq!(
        std::fs::read_to_string(work.join("a.txt")).unwrap(),
        "two\n"
    );
    assert!(!work.join("new.txt").exists());
    git(&[
        "rev-parse",
        "--verify",
        "-q",
        "refs/anchorleg/run-2/before-rewind",
    ]);

    // The first run has nothing before it.
    env.anchorleg()
        .args(["rewind", "1", "--", "again"])
        .assert()
        .failure();

    // Codex: the thread is forked through #1's last turn.
    let env = Env::mixed(&[("c", "codex")]);
    env.script("c.1.jsonl", &codex_ok("t1", "first"));
    env.script("c.2.jsonl", &codex_ok("t1", "second"));
    env.script("c.3.jsonl", &codex_ok("t1-fork", "the other way"));
    assert_eq!(env.run(&[]).0, 0);
    assert_eq!(env.run(&["--follow-up", "1"]).0, 0);
    let out = env
        .anchorleg()
        .args([
            "rewind",
            "2",
            "--keep-files",
            "--json",
            "--",
            "another",
            "way",
        ])
        .output()
        .unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["result"], "the other way", "{r}");
    let launches = env.launches();
    assert!(
        launches[2].contains("thread/fork t1") && launches[2].contains(" at=u1 "),
        "{launches:?}"
    );
}

#[test]
fn sync_mcp_adds_missing_servers_through_each_cli() {
    let env = Env::mixed(&[("a", "claude"), ("c", "codex"), ("g", "antigravity")]);
    std::fs::write(
        env.path("cfg/a/.claude.json"),
        r#"{"mcpServers":{"docs":{"command":"npx","args":["-y","docs-mcp"]},
            "api":{"type":"http","url":"https://api.example/mcp","headers":{"Authorization":"Bearer t"}}}}"#,
    )
    .unwrap();
    env.script(
        "c.mcp.json",
        r#"[{"name":"search","enabled":true,"transport":{"type":"streamable_http","url":"https://s.example/mcp"}}]"#,
    );
    // Without --yes: the plan only.
    let out = env.anchorleg().args(["sync", "mcp"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("add   docs → c"), "{text}");
    assert!(text.contains("skip  api → c"), "{text}");
    assert!(!env.path("fake/mcp.log").exists());

    env.anchorleg()
        .args(["sync", "mcp", "--yes"])
        .assert()
        .success();
    let log = std::fs::read_to_string(env.path("fake/mcp.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert!(
        lines.contains(&r#"a mcp add-json --scope user search {"headers":{},"type":"http","url":"https://s.example/mcp"}"#),
        "{log}"
    );
    assert!(
        lines.contains(&"c mcp add docs -- npx -y docs-mcp"),
        "{log}"
    );
    assert!(
        lines.contains(&"g mcp add docs -- npx -y docs-mcp"),
        "{log}"
    );
    assert!(
        lines.contains(&"g mcp add search https://s.example/mcp"),
        "{log}"
    );
    // The token-carrying server stays where it is.
    assert!(!log.contains("api"), "{log}");
}
