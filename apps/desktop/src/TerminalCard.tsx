import { useEffect, useState } from "react";
import { linkTerminal, terminal, type Terminal } from "./api";

/** "Use anchorleg in Terminal": links ~/.local/bin/anchorleg to the app's own CLI (D31). */
export default function TerminalCard({ onClose }: { onClose: () => void }) {
  const [t, setT] = useState<Terminal | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    terminal().then(setT, (e) => setError(String(e)));
  }, []);

  const link = async () => {
    setBusy(true);
    setError(null);
    try {
      setT(await linkTerminal());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Another anchorleg earlier on the shell's PATH wins over the link.
  const shadowed =
    t && t.state === "linked" && t.shell_resolves && t.shell_resolves !== t.link ? t.shell_resolves : null;

  return (
    <section className="card terminal">
      <div className="card-head">
        <h2>Use anchorleg in Terminal</h2>
        <button className="link" onClick={onClose} aria-label="Close">
          Close
        </button>
      </div>
      <p className="muted">
        Optional. The app already includes the anchorleg CLI. This puts the <code>anchorleg</code> command in your
        terminal too, as a link to the app&apos;s copy, so it updates with the app.
      </p>
      {!t && !error && <p className="muted">Checking…</p>}
      {t?.state === "unavailable" && <p className="muted">Not available: {t.reason}.</p>}
      {t?.state === "missing" && (
        <button onClick={link} disabled={busy}>
          Add <code>anchorleg</code> to Terminal
        </button>
      )}
      {t?.state === "older" && (
        <>
          <p>
            <code>{t.link}</code> is {t.version ? `anchorleg ${t.version}` : "a broken link"}; the app has{" "}
            {t.app_version}.
          </p>
          <button onClick={link} disabled={busy}>
            Replace it with the app&apos;s version
          </button>
        </>
      )}
      {t?.state === "other" && (
        <p>
          <code>{t.link}</code> is anchorleg {t.version ?? "(unknown version)"}, as new as the app&apos;s or newer, so
          it&apos;s left as it is.
        </p>
      )}
      {t?.state === "linked" && (
        <p className="ok">
          Done: <code>{t.link}</code> runs the app&apos;s anchorleg {t.app_version}.
        </p>
      )}
      {t?.needs_path && t.state !== "unavailable" && (
        <div>
          <p>
            <code>~/.local/bin</code> isn&apos;t on your shell&apos;s PATH yet. Add this to your shell profile:
          </p>
          <pre>export PATH="$HOME/.local/bin:$PATH"</pre>
        </div>
      )}
      {shadowed && (
        <p>
          Your terminal finds another anchorleg first: <code>{shadowed}</code>
          {t?.shell_version && ` (${t.shell_version})`}. Remove it, or put <code>~/.local/bin</code> earlier on PATH.
        </p>
      )}
      {error && <p className="error">{error}</p>}
    </section>
  );
}
