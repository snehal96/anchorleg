// Accounts and their quota (`anchorleg status --json` joined with `accounts list --json`), and
// everything `anchorleg accounts set / add / login / rm / import-aliases` can change.
import { useEffect, useRef, useState } from "react";
import {
  accounts as listAccounts,
  addAccount,
  cancelLogin,
  importAliases,
  loginAccount,
  loginInTerminal,
  removeAccount,
  setAccounts,
  type Account,
  type AccountChange,
  type AccountStatus,
  type Status,
  type WindowStatus,
} from "./api";
import { ago } from "./sessions";

/** A reading older than this shows its age, as the CLI does. */
const STALE_SECS = 3600;

export function windowName(w: string): string {
  return ({ five_hour: "5h", seven_day: "7d", seven_day_opus: "7d Opus", seven_day_sonnet: "7d Sonnet" } as Record<string, string>)[w] ?? w.replace(/_/g, " ");
}

export function until(secs: number, now = Date.now() / 1000): string {
  const d = Math.max(0, secs - now);
  if (d < 3600) return `${Math.ceil(d / 60)}m`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ${Math.floor((d % 3600) / 60)}m`;
  return `${Math.floor(d / 86400)}d ${Math.floor((d % 86400) / 3600)}h`;
}

export default function Accounts({
  status,
  list,
  refreshing,
  onRefresh,
  onChanged,
}: {
  status: Status | null;
  list: Account[] | null;
  refreshing: boolean;
  /** Ask every account's CLI for its quota now. */
  onRefresh: () => void;
  onChanged: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const run = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(String(e));
    }
    onChanged();
  };

  if (!status || !list) return <p className="muted pad">Loading…</p>;
  const byName = new Map(list.map((a) => [a.name, a]));
  // `status` is in the order accounts are tried.
  const rows = status.accounts;
  const move = (i: number, by: number) => {
    const order = rows.map((r) => r.name);
    const [n] = order.splice(i, 1);
    order.splice(i + by, 0, n);
    run(() => setAccounts(order.map((name, k) => ({ name, priority: k + 1 }))));
  };

  return (
    <div className="page">
      <div className="page-head">
        <h2>Accounts</h2>
        <Next status={status} />
        <span className="spacer" />
        <button className="secondary" disabled={refreshing} onClick={onRefresh} title="Ask each account's CLI for its quota now (no model request)">
          {refreshing ? "Reading quota…" : "Refresh quota"}
        </button>
      </div>
      {error && <p className="error">{error}</p>}
      {rows.length === 0 && <p className="muted">No accounts yet. Add one below, or import your shell aliases.</p>}
      <div className="accounts">
        {rows.map((s, i) => (
          <AccountRow
            key={s.name}
            s={s}
            a={byName.get(s.name)}
            next={status.next.action !== "no_accounts" && status.next.account === s.name}
            open={open === s.name}
            onToggle={() => setOpen(open === s.name ? null : s.name)}
            onUp={i > 0 ? () => move(i, -1) : undefined}
            onDown={i < rows.length - 1 ? () => move(i, 1) : undefined}
            onSet={(c) => run(() => setAccounts([{ ...c, name: s.name }]))}
            onRemove={() => run(() => removeAccount(s.name))}
          />
        ))}
      </div>
      <SignIn existing={list.map((a) => a.name)} onAdded={onChanged} />
      <AddAccount run={run} />
    </div>
  );
}

function Next({ status }: { status: Status }) {
  const n = status.next;
  if (n.action === "no_accounts") return <span className="muted">no account can run a task</span>;
  if (n.action === "wait") return <span className="muted">every account is out; {n.account} frees up in {until(n.until)}</span>;
  return (
    <span className="muted">
      next run: <b>{n.account}</b> ({status.policy}
      {n.reason !== "available" ? `, ${n.reason.replace("_", " ")}` : ""})
    </span>
  );
}

function AccountRow({
  s,
  a,
  next,
  open,
  onToggle,
  onUp,
  onDown,
  onSet,
  onRemove,
}: {
  s: AccountStatus;
  a: Account | undefined;
  next: boolean;
  open: boolean;
  onToggle: () => void;
  onUp?: () => void;
  onDown?: () => void;
  onSet: (c: Omit<AccountChange, "name">) => void;
  onRemove: () => void;
}) {
  const enabled = a?.enabled ?? true;
  const now = Date.now() / 1000;
  const blocked = s.blocked_until != null && s.blocked_until > now;
  return (
    <div className={`account ${enabled ? "" : "disabled"} ${next ? "next" : ""}`}>
      <div className="account-head">
        <span className="order">
          <button className="link" disabled={!onUp} onClick={onUp} aria-label="Move up">
            ▲
          </button>
          <button className="link" disabled={!onDown} onClick={onDown} aria-label="Move down">
            ▼
          </button>
        </span>
        <span className="account-name">
          <b>{s.name}</b> <span className="muted">{s.vendor}</span>
          {a?.pinned && <span className="tag">pinned</span>}
          {!enabled && <span className="tag">off</span>}
          {blocked && <span className="tag warn">blocked {until(s.blocked_until!)}</span>}
          {a?.stop_at != null && (
            <span className="tag" title="This account's own limit">
              stop at {Math.round(a.stop_at * 100)}%{a.strict ? " (strict)" : ""}
            </span>
          )}
        </span>
        <span className="meters">
          {s.windows.length === 0 && <span className="muted small">no quota read yet</span>}
          {s.windows.map((w) => (
            <Meter key={w.window} w={w} stopAt={a?.stop_at} reserve={a?.reserve_7d} />
          ))}
        </span>
        {s.credits && (
          <span className={`tag ${s.credits.state === "in_use" ? "warn" : ""}`} title={s.credits.reason ?? ""}>
            credits {s.credits.state.replace("_", " ")}
            {s.credits.balance != null ? ` $${s.credits.balance.toFixed(2)}` : ""}
          </span>
        )}
        <button className="link" onClick={onToggle}>
          {open ? "Done" : "Edit"}
        </button>
      </div>
      {open && <Editor s={s} a={a} onSet={onSet} onRemove={onRemove} />}
    </div>
  );
}

function Meter({ w, stopAt, reserve }: { w: WindowStatus; stopAt?: number; reserve?: number }) {
  const used = w.used ?? 0;
  const pct = Math.round(used * 100);
  const stale = Date.now() / 1000 - w.updated_at > STALE_SECS;
  const limit = Math.min(stopAt ?? 1, w.window.startsWith("seven_day") && reserve ? 1 - reserve : 1);
  const level = used >= 0.95 ? "full" : used >= limit || used >= 0.8 ? "high" : "ok";
  const title = [
    `${windowName(w.window)}: ${w.used == null ? "unknown" : `${pct}% used`}`,
    w.resets_at ? `resets in ${until(w.resets_at)}` : null,
    `read ${ago(w.updated_at)} ago from ${w.source}`,
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <span className={`meter ${stale ? "stale" : ""}`} title={title}>
      <span className="meter-label">{windowName(w.window)}</span>
      <span className="bar">
        <span className={`fill ${level}`} style={{ width: `${Math.min(100, pct)}%` }} />
        {limit < 1 && <span className="limit" style={{ left: `${limit * 100}%` }} />}
      </span>
      <span className="meter-value mono">{w.used == null ? "?" : `${pct}% used`}</span>
      {stale && <span className="muted small">read {ago(w.updated_at)} ago</span>}
    </span>
  );
}

const pctOf = (v?: number) => (v == null ? "" : String(Math.round(v * 100)));

function Editor({ s, a, onSet, onRemove }: { s: AccountStatus; a: Account | undefined; onSet: (c: Omit<AccountChange, "name">) => void; onRemove: () => void }) {
  const [stopAt, setStopAt] = useState(pctOf(a?.stop_at));
  const [reserve, setReserve] = useState(pctOf(a?.reserve_7d));
  const [confirm, setConfirm] = useState(false);
  // An empty box turns the limit off (0).
  const frac = (v: string) => (v.trim() === "" ? 0 : Math.min(100, Math.max(0, Number(v))) / 100);
  return (
    <div className="editor">
      <label>
        <input type="checkbox" checked={a?.enabled ?? true} onChange={(e) => onSet({ enabled: e.target.checked })} /> Use this account
      </label>
      <label>
        <input type="checkbox" checked={a?.pinned ?? false} onChange={(e) => onSet({ pinned: e.target.checked })} /> Pinned: use it first
        whenever it has room
      </label>
      <label title="A session on it stops once any window reaches this and moves to another account">
        Own limit
        <input className="num" inputMode="numeric" placeholder="off" value={stopAt} onChange={(e) => setStopAt(e.target.value)} onBlur={() => onSet({ stop_at: frac(stopAt) })} />%
      </label>
      <label title="Never go past the limit: wait for the window to reset instead">
        <input type="checkbox" checked={a?.strict ?? false} disabled={a?.stop_at == null} onChange={(e) => onSet({ strict: e.target.checked })} /> Strict
      </label>
      <label title="Keep this share of the weekly window for when nothing else is free">
        Weekly reserve
        <input className="num" inputMode="numeric" placeholder="off" value={reserve} onChange={(e) => setReserve(e.target.value)} onBlur={() => onSet({ reserve_7d: Math.min(0.99, frac(reserve)) })} />%
      </label>
      <label title="Paid usage past the plan's limits">
        Credits
        <select value={s.credits_use} onChange={(e) => onSet({ credits: e.target.value })}>
          <option value="never">never spend</option>
          <option value="last-resort">only when every account is out</option>
          <option value="allow">allow</option>
        </select>
      </label>
      <div className="editor-foot">
        <span className="muted small mono" title="How anchorleg launches it">
          {a?.shell ? `shell: ${a.shell}` : a?.bin ?? ""}
          {a?.token_from ? " · token in Keychain" : ""}
        </span>
        <span className="spacer" />
        {confirm ? (
          <>
            <span className="small">Remove {s.name}{a?.token_from ? " and its Keychain token" : ""}?</span>
            <button className="danger small" onClick={onRemove}>
              Remove
            </button>
            <button className="secondary small" onClick={() => setConfirm(false)}>
              Keep
            </button>
          </>
        ) : (
          <button className="link" onClick={() => setConfirm(true)}>
            Remove…
          </button>
        )}
      </div>
    </div>
  );
}

/** A new account in one step: a name, which agent, and the agent's own sign-in in the browser. */
function SignIn({ existing, onAdded }: { existing: string[]; onAdded: () => void }) {
  const [name, setName] = useState("");
  const [vendor, setVendor] = useState("claude");
  const [state, setState] = useState<"idle" | "waiting" | "done">("idle");
  const [msg, setMsg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const clean = name.trim();
  const bad = clean !== "" && !/^[A-Za-z0-9_-]+$/.test(clean) ? "Letters, digits, - and _ only" : existing.includes(clean) ? "That name is taken" : null;
  const folder = `~/.${vendor === "codex" ? "codex" : vendor === "antigravity" ? "agy" : "claude"}-${clean || "name"}`;
  // Antigravity signs in from its own screen in a Terminal window: watch for the account.
  const [terminalFor, setTerminalFor] = useState<string | null>(null);
  const added = useRef(onAdded);
  added.current = onAdded;
  useEffect(() => {
    if (!terminalFor) return;
    const started = Date.now();
    const t = setInterval(async () => {
      const list = await listAccounts().catch(() => null);
      if (list?.some((a) => a.name === terminalFor)) {
        clearInterval(t);
        setTerminalFor(null);
        setState("done");
        setMsg(`Signed in. ${terminalFor} is ready and will be used when it has room.`);
        setName("");
        added.current();
      } else if (Date.now() - started > 15 * 60_000) {
        clearInterval(t);
        setTerminalFor(null);
        setState("idle");
      }
    }, 3000);
    return () => clearInterval(t);
  }, [terminalFor]);
  const inTerminal = (n: string, v: string) => {
    setError(null);
    setMsg(null);
    loginInTerminal(n, v).then(
      () => (setState("waiting"), setTerminalFor(n)),
      (e) => (setState("idle"), setError(String(e))),
    );
  };
  const go = async () => {
    if (vendor === "antigravity") return inTerminal(clean, vendor);
    setState("waiting");
    setError(null);
    setMsg(null);
    try {
      const r = await loginAccount(clean, vendor);
      setState("done");
      setMsg(`Signed in. ${r.name} is ready and will be used when it has room.`);
      setName("");
      onAdded();
    } catch (e) {
      setState("idle");
      if (String(e) !== "sign-in cancelled") setError(String(e));
    }
  };
  return (
    <section className="card sign-in">
      <h3>Add an account</h3>
      <p className="muted small">
        Sign in to another subscription. It gets its own folder (<code>{folder}</code>) and the agent's own sign-in page opens in your browser. Your
        login stays with the agent; anchorleg never sees it.
      </p>
      <div className="add-row">
        <div className="segmented" role="radiogroup" aria-label="Agent">
          {[
            ["claude", "Claude Code"],
            ["codex", "Codex"],
            ["antigravity", "Antigravity"],
          ].map(([v, l]) => (
            <button key={v} role="radio" aria-checked={vendor === v} className={vendor === v ? "on" : ""} disabled={state === "waiting"} onClick={() => setVendor(v)}>
              {l}
            </button>
          ))}
        </div>
        <input placeholder="Name, e.g. work" value={name} disabled={state === "waiting"} onChange={(e) => (setName(e.target.value), setState("idle"))} />
        {state === "waiting" ? (
          <button className="secondary" onClick={() => (terminalFor ? (setTerminalFor(null), setState("idle")) : cancelLogin())}>
            Cancel
          </button>
        ) : (
          <button className="primary" disabled={!clean || bad != null} onClick={go}>
            Sign in…
          </button>
        )}
      </div>
      {bad && <p className="error small">{bad}</p>}
      {state === "waiting" && terminalFor && (
        <p className="sign-in-wait small">
          <span className="spinner" /> A Terminal window opened with Antigravity: sign in there (it opens your browser), then quit agy. The account
          shows up here by itself.
        </p>
      )}
      {state === "waiting" && !terminalFor && (
        <p className="sign-in-wait small">
          <span className="spinner" /> Finish signing in in your browser, then come back here.{" "}
          <button className="link" onClick={() => (cancelLogin(), inTerminal(clean, vendor))}>
            Do it in Terminal instead
          </button>
        </p>
      )}
      {msg && <p className="ok small">{msg}</p>}
      {error && (
        <p className="error small">
          {error}{" "}
          {clean && (
            <button className="link" onClick={() => inTerminal(clean, vendor)}>
              Try in Terminal
            </button>
          )}
        </p>
      )}
    </section>
  );
}

function AddAccount({ run }: { run: (f: () => Promise<unknown>) => Promise<void> }) {
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [preview, setPreview] = useState<{ add: Account[]; skip: string[] } | null>(null);
  const [note, setNote] = useState<string | null>(null);
  return (
    <section className="card add-account">
      <h3>Add with a command</h3>
      <p className="muted small">
        For an account you already use from a terminal. The command you'd type to run it, e.g. <code>CLAUDE_CONFIG_DIR=~/.claude-work claude</code> or <code>CODEX_HOME=~/.codex-2 codex</code>. For
        a token from <code>claude setup-token</code>, use <code>anchorleg accounts add NAME --token-stdin</code> in a terminal; the token goes
        straight to the Keychain.
      </p>
      <div className="add-row">
        <input placeholder="name" value={name} onChange={(e) => setName(e.target.value)} />
        <input className="mono grow" placeholder="command" value={command} onChange={(e) => setCommand(e.target.value)} />
        <button
          disabled={!name.trim() || !command.trim()}
          onClick={() =>
            run(async () => {
              await addAccount(name.trim(), command.trim());
              setName("");
              setCommand("");
            })
          }
        >
          Add
        </button>
      </div>
      <div className="add-row">
        <button
          className="secondary"
          onClick={() =>
            run(async () => {
              const p = await importAliases(false);
              setPreview(p);
              setNote(p.add.length ? null : "No new shell aliases that run claude, codex, kimi or cursor-agent.");
            })
          }
        >
          Import from shell aliases…
        </button>
        {note && <span className="muted small">{note}</span>}
      </div>
      {preview && preview.add.length > 0 && (
        <div className="import">
          <ul>
            {preview.add.map((a) => (
              <li key={a.name} className="mono small">
                {a.name} → {Object.entries(a.env ?? {}).map(([k, v]) => `${k}=${/TOKEN|KEY|SECRET|PASS/i.test(k) ? "***" : v} `)}
                {a.bin} {(a.args ?? []).join(" ")}
              </li>
            ))}
            {preview.skip.map((n) => (
              <li key={n} className="muted small">
                {n}: already an account
              </li>
            ))}
          </ul>
          <button
            onClick={() =>
              run(async () => {
                await importAliases(true);
                setPreview(null);
              })
            }
          >
            Add {preview.add.length} account{preview.add.length > 1 ? "s" : ""}
          </button>{" "}
          <button className="secondary" onClick={() => setPreview(null)}>
            Cancel
          </button>
        </div>
      )}
    </section>
  );
}
