//! anchorleg-mod, the Claude Code mod in `mod/`, embedded in the binary so an installed `anchorleg`
//! carries it. Written out once per content version and passed to `claude --plugin-dir`.

use std::path::{Path, PathBuf};

/// The mod's files (tests left out), by path inside the plugin folder.
const FILES: &[(&str, &str)] = &[
    (
        ".claude-plugin/plugin.json",
        include_str!("../../../mod/.claude-plugin/plugin.json"),
    ),
    (
        "hooks/hooks.json",
        include_str!("../../../mod/hooks/hooks.json"),
    ),
    (
        "hooks/register.ts",
        include_str!("../../../mod/hooks/register.ts"),
    ),
];

/// FNV-1a over every file, so a changed mod lands in a new folder.
fn content_hash() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (path, text) in FILES {
        for b in path.bytes().chain(text.bytes()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Write the mod under `<base>/mod/<hash>/` if it isn't there yet; returns that folder.
pub fn install(base: &Path) -> std::io::Result<PathBuf> {
    let dir = base.join("mod").join(content_hash());
    for (path, text) in FILES {
        let file = dir.join(path);
        if std::fs::read_to_string(&file).ok().as_deref() == Some(*text) {
            continue;
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, text)?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_once_per_version() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install(tmp.path()).unwrap();
        assert!(dir.join("hooks/register.ts").is_file());
        assert!(dir.join(".claude-plugin/plugin.json").is_file());
        assert_eq!(install(tmp.path()).unwrap(), dir);
    }
}
