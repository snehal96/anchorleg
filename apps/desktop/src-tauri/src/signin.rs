//! Signing in a new account from the window: `anchorleg accounts login NAME --cli C --json`,
//! which runs the CLI's own sign-in (it opens the browser) and adds the account once it's done.
//! The login stays with the CLI; nothing here sees it. One sign-in at a time; the window can
//! cancel it, and it gives up after 10 minutes.

use std::io::Read as _;
use std::process::Child;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::{blocking, cli};

const LIMIT: Duration = Duration::from_secs(600);

static CURRENT: Mutex<Option<Child>> = Mutex::new(None);

#[tauri::command]
pub async fn login_account(name: String, vendor: String) -> Result<Value, String> {
    blocking(move || {
        let child = cli::spawn(&["accounts", "login", &name, "--cli", &vendor, "--json"])?;
        {
            let mut cur = CURRENT.lock().map_err(|e| e.to_string())?;
            if let Some(mut old) = cur.take() {
                let _ = old.kill();
            }
            *cur = Some(child);
        }
        let started = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let mut cur = CURRENT.lock().map_err(|e| e.to_string())?;
            let Some(child) = cur.as_mut() else {
                return Err("sign-in cancelled".into());
            };
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                let mut child = cur.take().expect("checked");
                let (mut out, mut err) = (String::new(), String::new());
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                // Already exited (try_wait said so): this only collects it.
                let _ = child.wait();
                if !status.success() {
                    let last = err.trim().lines().last().unwrap_or("sign-in failed");
                    return Err(last.trim_start_matches("anchorleg: ").to_owned());
                }
                return serde_json::from_str(&out)
                    .map_err(|e| format!("reading anchorleg's output: {e}"));
            }
            if started.elapsed() > LIMIT {
                if let Some(mut child) = cur.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err("the sign-in wasn't finished within 10 minutes".into());
            }
        }
    })
    .await
}

#[tauri::command]
pub fn cancel_login() {
    if let Ok(mut cur) = CURRENT.lock()
        && let Some(mut child) = cur.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The same sign-in in a Terminal window, for when the CLI wants to ask something there.
#[tauri::command]
pub async fn login_in_terminal(name: String, vendor: String) -> Result<(), String> {
    blocking(move || {
        let bin = cli::find().ok_or("the anchorleg CLI isn't installed")?;
        let ok = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok(&name) || !ok(&vendor) {
            return Err("an account name is letters, digits, `-` and `_`".into());
        }
        let script = std::env::temp_dir().join(format!("anchorleg-login-{name}.command"));
        let body = format!(
            "#!/bin/sh\n'{}' accounts login {name} --cli {vendor}\necho\necho 'Done. You can close this window and go back to anchorleg.'\n",
            bin.display().to_string().replace('\'', "'\\''")
        );
        std::fs::write(&script, body).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        let status = std::process::Command::new("open")
            .args(["-a", "Terminal"])
            .arg(&script)
            .status()
            .map_err(|e| format!("opening Terminal: {e}"))?;
        if status.success() { Ok(()) } else { Err("couldn't open Terminal".into()) }
    })
    .await
}
