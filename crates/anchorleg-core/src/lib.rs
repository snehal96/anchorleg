//! anchorleg-core: everything anchorleg does apart from argument parsing and the terminal UI.
//!
//! Modules: adapters (one per vendor CLI), store, registry, policy, supervisor, handoff,
//! checkpoint, conversation.

pub mod adapters;
pub mod anchorleg_mod;
pub mod checkpoint;
pub mod conversation;
pub mod handoff;
pub mod output;
pub mod policy;
pub mod registry;
pub mod store;
pub mod supervisor;

/// Version of the anchorleg crates, reported by `anchorleg --version` and in `--json` output.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The project was called relay before 0.1 was published. If `new` doesn't exist yet and the old
/// folder does, move it over (and rename files in it, e.g. `relay.db` → `anchorleg.db`) so
/// existing accounts, sessions and quota carry on. Best effort: on any error the new folder just
/// starts empty.
pub(crate) fn adopt_legacy_dir(
    old: &std::path::Path,
    new: &std::path::Path,
    files: &[(&str, &str)],
) {
    if new.exists() || !old.is_dir() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::rename(old, new).is_ok() {
        for (from, to) in files {
            let _ = std::fs::rename(new.join(from), new.join(to));
        }
        tracing::info!("moved {} to {}", old.display(), new.display());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_folder_is_adopted_once() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("relay");
        let new = dir.path().join("anchorleg");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("relay.db"), "x").unwrap();
        super::adopt_legacy_dir(&old, &new, &[("relay.db", "anchorleg.db")]);
        assert!(!old.exists());
        assert_eq!(
            std::fs::read_to_string(new.join("anchorleg.db")).unwrap(),
            "x"
        );
        // A second old folder appearing later is left alone.
        std::fs::create_dir_all(&old).unwrap();
        super::adopt_legacy_dir(&old, &new, &[]);
        assert!(old.exists());
    }
}
