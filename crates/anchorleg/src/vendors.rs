//! `anchorleg vendors [--json]`: what each CLI can do with anchorleg, and whether it's installed.

use anchorleg_core::registry::{Vendor, default_program};
use anchorleg_core::vendors::{QuotaSource, SignIn};
use serde_json::{Value, json};

pub fn run(json: bool) -> anyhow::Result<()> {
    let rows: Vec<Value> = Vendor::ALL
        .into_iter()
        .map(|v| {
            let mut row = serde_json::to_value(v.caps()).expect("caps serialize");
            row["bin"] = json!(v.default_bin());
            row["installed"] = json!(installed(v));
            row["efforts"] = json!(v.efforts());
            row
        })
        .collect();
    if json {
        println!("{}", json!({ "schema": 1, "vendors": rows }));
        return Ok(());
    }
    for v in Vendor::ALL {
        let c = v.caps();
        let mut can = Vec::new();
        for (on, what) in [
            (c.resume, "resume"),
            (c.fork, "fork"),
            (c.side, "side questions"),
            (c.rewind, "rewind"),
            (c.plan, "plan"),
            (c.approvals, "approvals"),
            (c.questions, "questions"),
            (c.json_schema, "json-schema"),
        ] {
            if on {
                can.push(what);
            }
        }
        let quota = match c.quota {
            QuotaSource::Live => "quota read live",
            QuotaSource::Runs => "quota from runs",
            QuotaSource::None => "no quota reading (switches on a limit error)",
        };
        let sign_in = match c.sign_in {
            SignIn::Browser { .. } => "sign in: accounts login (browser)",
            SignIn::Terminal { .. } => "sign in: accounts login (in a terminal)",
            SignIn::Manual => "sign in: with its own app, then accounts add",
        };
        println!(
            "{:<12} {:<12} {}{}",
            v.name(),
            c.label,
            if installed(v) { "" } else { "(not installed) " },
            if c.driven {
                format!("{quota}; {}; {sign_in}", can.join(", "))
            } else {
                "not supported yet".to_owned()
            }
        );
    }
    Ok(())
}

/// The CLI's binary resolves: an absolute path that exists, or a name on `PATH`.
fn installed(v: Vendor) -> bool {
    let program = default_program(v);
    let path = std::path::Path::new(&program);
    if path.is_absolute() {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(&program).is_file()))
}
