import { useEffect, useState } from "react";
import { linkTerminal, openTarget, terminal, type Terminal } from "./api";
import { Modal } from "./ui";

/** Where people get a newer app: the desktop releases, never the CLI's "latest". */
export const DESKTOP_DOWNLOADS = "https://github.com/snehal96/anchorleg/releases?q=desktop&expanded=true";

/** What the CLI says when the database is from a newer anchorleg than the one asking. */
export const isNewerDatabase = (error: string | null) => !!error && error.includes("database is from a newer anchorleg");

const DISMISSED = "anchorleg.terminalOutdated";

type V = [number, number, number];
const parse = (v: string | null | undefined): V | null => {
  const m = v && /(\d+)\.(\d+)\.(\d+)/.exec(v);
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
};
export const older = (a: string | null | undefined, b: string | null | undefined) => {
  const x = parse(a);
  const y = parse(b);
  if (!x || !y) return false;
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i];
  return false;
};

/** The `anchorleg` a terminal runs, when it's older than the app's own copy: its path and
 * version. `null` when it's as new, missing, or can't be told. */
export function outdated(t: Terminal): { path: string; version: string | null } | null {
  if (t.state === "unavailable" || !t.app_version) return null;
  if (t.shell_resolves && t.shell_version && older(t.shell_version, t.app_version)) {
    return { path: t.shell_resolves, version: t.shell_version };
  }
  if (t.state === "older") return { path: t.link, version: t.version };
  return null;
}

/** Asked once a launch: a terminal `anchorleg` older than the app's gets a dialog offering to
 * point it at the app's copy (the "Use in Terminal" link). "Not now" is remembered for this
 * pair of versions. */
export function TerminalOutdated() {
  const [t, setT] = useState<Terminal | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  useEffect(() => {
    terminal().then(
      (t) => {
        const o = outdated(t);
        if (!o) return;
        let seen: string | null = null;
        try {
          seen = localStorage.getItem(DISMISSED);
        } catch {
          /* asked again next time */
        }
        setT(t);
        setOpen(seen !== `${o.version}->${t.app_version}`);
      },
      () => {},
    );
  }, []);

  if (!t || !open) return null;
  const o = outdated(t);
  const close = () => setOpen(false);
  const later = () => {
    try {
      if (!done && o) localStorage.setItem(DISMISSED, `${o.version}->${t.app_version}`);
    } catch {
      /* asked again next time */
    }
    close();
  };
  // The link only helps when the terminal finds ~/.local/bin first (or nothing else).
  const linkWins = !t.shell_resolves || t.shell_resolves === t.link;
  const brew = o?.path.includes("homebrew") || o?.path.startsWith("/usr/local/Cellar");
  const update = async () => {
    setBusy(true);
    setError(null);
    try {
      const after = await linkTerminal();
      setT(after);
      if (outdated(after)) setError(`Your terminal still finds ${after.shell_resolves} first.`);
      else setDone(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={done ? "Terminal updated" : "Update anchorleg in Terminal?"} onClose={later}>
      {done ? (
        <p className="ok">
          Done: <code>anchorleg</code> in a new terminal runs the app&apos;s {t.app_version}, and updates with the app.
        </p>
      ) : (
        <>
          <p>
            Your terminal runs anchorleg {o?.version ?? "(unknown version)"} (<code>{o?.path}</code>); this app has{" "}
            {t.app_version}. Sessions you start from the terminal would use the older one, and it can&apos;t open your
            sessions once the app has updated them.
          </p>
          {linkWins ? (
            <p className="muted">
              Update points <code>{t.link}</code> at the app&apos;s copy, so it stays in step with the app.
            </p>
          ) : brew ? (
            <p className="muted">
              It was installed with Homebrew: run <code>brew upgrade anchorleg</code>, or <code>brew uninstall anchorleg</code>{" "}
              to use the app&apos;s copy instead.
            </p>
          ) : (
            <p className="muted">
              Your terminal finds <code>{t.shell_resolves}</code> before <code>{t.link}</code>. Update or remove that copy,
              or put <code>~/.local/bin</code> earlier on your PATH.
            </p>
          )}
          {t.needs_path && linkWins && (
            <p className="muted">
              Then add <code>export PATH=&quot;$HOME/.local/bin:$PATH&quot;</code> to your shell profile.
            </p>
          )}
        </>
      )}
      {error && <p className="error">{error}</p>}
      <div className="modal-actions">
        <button className="secondary" onClick={done ? close : later}>
          {done ? "Close" : "Not now"}
        </button>
        {!done && linkWins && (
          <button className="primary" disabled={busy} onClick={update}>
            {busy ? "…" : "Update"}
          </button>
        )}
      </div>
    </Modal>
  );
}

/** The app's own CLI is older than the data a newer anchorleg left: the app needs updating. */
export function AppOutdated({ onClose }: { onClose: () => void }) {
  return (
    <Modal title="Update the anchorleg app" onClose={onClose}>
      <p>
        A newer anchorleg (probably the one in your terminal) has updated your sessions&apos; data, and this app&apos;s
        copy is older, so it can&apos;t read them. Install the latest desktop app; your sessions are all still there.
      </p>
      <div className="modal-actions">
        <button className="secondary" onClick={onClose}>
          Not now
        </button>
        <button className="primary" onClick={() => openTarget(DESKTOP_DOWNLOADS).catch(() => {})}>
          Download the latest app
        </button>
      </div>
    </Modal>
  );
}
