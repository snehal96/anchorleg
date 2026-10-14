// The rest of the settings: lasting allow rules (`anchorleg rules`), notifications
// (`anchorleg notify`) and the same MCP servers everywhere (`anchorleg sync mcp`).
import { useEffect, useState } from "react";
import { mcpSync, notifySettings, rules, type McpSync, type NotifySettings } from "./api";

export function Rules() {
  const [list, setList] = useState<string[] | null>(null);
  const [rule, setRule] = useState("");
  const [error, setError] = useState<string | null>(null);
  const run = (f: () => Promise<{ rules: string[] }>) =>
    f().then(
      (r) => (setList(r.rules), setError(null)),
      (e) => setError(String(e)),
    );
  useEffect(() => {
    run(() => rules());
  }, []);
  return (
    <section className="card">
      <h3>Lasting rules</h3>
      <p className="muted small">
        Tool calls every session may make without asking, in Claude Code's rule syntax: <code>Bash(cargo test *)</code>,{" "}
        <code>Edit(src/**)</code>, <code>WebFetch(domain:docs.rs)</code>. Claude Code applies them itself; anchorleg applies them to
        Codex and Antigravity.
      </p>
      {error && <p className="error small">{error}</p>}
      {list && list.length === 0 && <p className="muted small">No rules: every tool call the CLI asks about comes to you.</p>}
      <ul className="rules">
        {list?.map((r) => (
          <li key={r}>
            <code>{r}</code>
            <button className="link" onClick={() => run(() => rules([], [r]))}>
              Remove
            </button>
          </li>
        ))}
      </ul>
      <div className="add-row">
        <input
          className="mono grow"
          placeholder="Bash(npm run test:*)"
          value={rule}
          onChange={(e) => setRule(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && rule.trim() && run(() => rules([rule.trim()])).then(() => setRule(""))}
        />
        <button disabled={!rule.trim()} onClick={() => run(() => rules([rule.trim()])).then(() => setRule(""))}>
          Add
        </button>
      </div>
    </section>
  );
}

export function Notifications() {
  const [n, setN] = useState<NotifySettings | null>(null);
  const [command, setCommand] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = (change?: Parameters<typeof notifySettings>[0], msg?: string) =>
    notifySettings(change).then(
      (s) => (setN(s), setCommand(s.command ?? ""), setError(null), setNote(msg ?? null)),
      (e) => setError(String(e)),
    );
  useEffect(() => {
    run();
  }, []);
  if (!n) return null;
  return (
    <section className="card">
      <h3>Notifications</h3>
      <p className="muted small">
        When a run needs you (a tool call, a question, a plan) or ends, switches or runs out. While this app is open it shows them;
        otherwise each run sends its own.
      </p>
      {error && <p className="error small">{error}</p>}
      <label className="option">
        <input type="checkbox" checked={n.desktop} onChange={(e) => run({ desktop: e.target.checked })} /> Desktop notifications
      </label>
      <div className="add-row">
        <input
          className="mono grow"
          placeholder={'also run a command, e.g. curl -s -d "$ANCHORLEG_MESSAGE" https://ntfy.sh/my-topic'}
          value={command}
          onChange={(e) => setCommand(e.target.value)}
        />
        <button className="secondary" disabled={command === (n.command ?? "")} onClick={() => run({ command }, command ? "saved" : "command removed")}>
          Save
        </button>
        <button className="secondary" onClick={() => run({ test: true }, "test notice sent")}>
          Send a test
        </button>
      </div>
      <p className="muted small">The command gets the notice as JSON on stdin and as ANCHORLEG_EVENT, ANCHORLEG_RUN_ID, ANCHORLEG_TITLE, ANCHORLEG_MESSAGE.</p>
      {note && <p className="ok small">{note}</p>}
    </section>
  );
}

export function McpServers() {
  const [withEnv, setWithEnv] = useState(false);
  const [plan, setPlan] = useState<McpSync | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const load = async (yes: boolean) => {
    setBusy(true);
    setError(null);
    try {
      setPlan(await mcpSync(withEnv, yes));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const adds = plan?.plan.filter((a) => !a.skip) ?? [];
  return (
    <section className="card">
      <h3>MCP servers on every account</h3>
      <p className="muted small">
        Reads each account's MCP servers from its own config and adds the ones another account lacks, through that CLI's own{" "}
        <code>mcp add</code>.
      </p>
      <div className="add-row">
        <button className="secondary" disabled={busy} onClick={() => load(false)}>
          {busy ? "Reading…" : "Compare accounts"}
        </button>
        <label className="option small" title="Servers with env values or headers often hold tokens">
          <input type="checkbox" checked={withEnv} onChange={(e) => setWithEnv(e.target.checked)} /> include servers with env values or headers
        </label>
      </div>
      {error && <p className="error small">{error}</p>}
      {plan && (
        <>
          <ul className="small">
            {plan.accounts.map((a) => (
              <li key={a.name}>
                <b>{a.name}</b>: {a.servers.length ? a.servers.join(", ") : <span className="muted">none</span>}
              </li>
            ))}
          </ul>
          {plan.plan.length === 0 && <p className="ok small">Every account has the same MCP servers.</p>}
          {plan.plan.length > 0 && (
            <ul className="small">
              {plan.plan.map((a, i) => {
                const r = plan.results?.find((x) => x.server === a.server && x.account === a.account);
                return (
                  <li key={i} className={a.skip ? "muted" : ""}>
                    {a.server} → {a.account}
                    {a.skip ? `: skipped (${a.skip})` : r ? (r.ok ? ": added" : `: failed (${r.error})`) : ""}
                  </li>
                );
              })}
            </ul>
          )}
          {adds.length > 0 && !plan.results && (
            <button disabled={busy} onClick={() => load(true)}>
              Add {adds.length} server{adds.length > 1 ? "s" : ""}
            </button>
          )}
        </>
      )}
    </section>
  );
}
