import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import {
  accounts,
  cliInfo,
  permissions,
  sessions,
  status,
  updateTray,
  type Account,
  type CliInfo,
  type Notice,
  type Request,
  type Run,
  type Status,
} from "./api";
import Accounts, { windowName } from "./Accounts";
import Composer from "./Composer";
import Settings from "./Settings";
import Usage from "./Usage";
import Conversation from "./Conversation";
import { ago, heads, isWorking, nameOf, newest, tilde } from "./sessions";
import TerminalCard from "./TerminalCard";

const POLL_MS = 2000;
const TERMINAL_SEEN = "terminal-card-seen";
const LAST_FOLDER = "last-folder";
/** Coming back to the window this soon after a notice opens its session (the notification was clicked). */
const NOTICE_FOCUS_MS = 20_000;
/** Quota readings come from the store (cheap); `status --refresh` asks the CLIs. */
const STATUS_POLL_MS = 15_000;

type Page = "sessions" | "accounts" | "usage" | "settings";
const PAGES: [Page, string][] = [
  ["sessions", "Sessions"],
  ["accounts", "Accounts"],
  ["usage", "Usage"],
  ["settings", "Settings"],
];

/** The menu bar item: the fullest window of the account the next run would use, and one line per account. */
function trayOf(st: Status, waiting: number): { title: string | null; lines: string[] } {
  const fullest = (a: Status["accounts"][number]) => Math.max(0, ...a.windows.map((w) => w.used ?? 0));
  const n = st.next;
  const next = n.action === "no_accounts" ? null : st.accounts.find((a) => a.name === n.account);
  const lines = st.accounts.map(
    (a) =>
      `${a.name}  ${a.windows.length ? a.windows.map((w) => `${windowName(w.window)} ${Math.round((w.used ?? 0) * 100)}%`).join(" · ") : "no quota read yet"}`,
  );
  const pct = next && next.windows.length ? `${Math.round(fullest(next) * 100)}%` : null;
  const title = waiting > 0 ? `! ${waiting}${pct ? ` · ${pct}` : ""}` : pct;
  return { title, lines };
}

/** Shown once on first launch; the header button opens it again. */
function firstLaunch(): boolean {
  try {
    if (localStorage.getItem(TERMINAL_SEEN)) return false;
    localStorage.setItem(TERMINAL_SEEN, "1");
    return true;
  } catch {
    return false;
  }
}

function remembered(key: string): string {
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function Logo() {
  return (
    <svg viewBox="0 0 512 512" width="22" height="22" aria-hidden="true">
      <defs>
        <linearGradient id="g" x1="214" x2="298" y1="0" y2="0" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#F2A93B" />
          <stop offset="1" stopColor="#2FB383" />
        </linearGradient>
      </defs>
      <rect width="512" height="512" rx="114" fill="#14202B" />
      <path d="M128 312 H384 M128 200 H384" stroke="#243241" strokeWidth="44" strokeLinecap="round" />
      <path d="M128 312 H206 C256 312 256 200 306 200 H352" fill="none" stroke="url(#g)" strokeWidth="44" strokeLinecap="round" />
      <path d="M346 164 L392 200 L346 236" fill="none" stroke="#2FB383" strokeWidth="36" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx="128" cy="312" r="34" fill="#14202B" stroke="#F2A93B" strokeWidth="20" />
    </svg>
  );
}

function Setup({ info }: { info: CliInfo }) {
  return (
    <section className="card setup">
      <h2>The anchorleg CLI is needed</h2>
      <p>
        {info.problem}
        {info.path && <span className="muted"> ({info.path})</span>}
      </p>
      <p>This app runs your sessions through the CLI. Install it (version {info.min_version} or newer) in a terminal:</p>
      <pre>{info.install_command}</pre>
      <p className="muted">Then come back here; the app checks again by itself.</p>
    </section>
  );
}

const MARK: Record<string, string> = { done: "✓", failed: "✗", stopped: "■" };

function Sidebar({
  runs,
  waiting,
  error,
  selected,
  onSelect,
}: {
  runs: Run[] | null;
  /** Runs with something waiting for the person. */
  waiting: Set<number>;
  error: string | null;
  selected: number | "new";
  onSelect: (s: number | "new") => void;
}) {
  return (
    <nav className="sidebar">
      <button className={`item new ${selected === "new" ? "selected" : ""}`} onClick={() => onSelect("new")}>
        + New session
      </button>
      {error && <p className="error pad">{error}</p>}
      {!runs && !error && <p className="muted pad">Loading…</p>}
      {runs && runs.length === 0 && <p className="muted pad">No sessions yet.</p>}
      {runs &&
        heads(runs).map((r) => {
          const working = isWorking(r);
          return (
            <button
              key={r.id}
              className={`item ${selected === r.id ? "selected" : ""}`}
              onClick={() => onSelect(r.id)}
              title={`${r.task}\n\n${r.cwd}`}
            >
              <span className={`mark state-${working ? "running" : r.state}`}>
                {waiting.has(r.id) ? <span className="waiting-badge">!</span> : working ? <span className="spinner" /> : (MARK[r.state] ?? "●")}
              </span>
              <span className="item-text">
                <span className="item-title">{nameOf(runs, r) ?? r.task.replace(/\s+/g, " ")}</span>
                <span className="item-meta muted">
                  #{r.id} · {tilde(r.cwd).split("/").pop()} · {ago(r.started_at)}
                </span>
              </span>
            </button>
          );
        })}
    </nav>
  );
}

function NewSession({ folder, setFolder, onStarted }: { folder: string; setFolder: (f: string) => void; onStarted: (run: number) => void }) {
  const choose = async () => {
    const picked = await open({ directory: true, defaultPath: folder || undefined });
    if (typeof picked === "string") setFolder(picked);
  };
  return (
    <div className="pane">
      <div className="pane-head">
        <h2>New session</h2>
        <div className="folder">
          <input className="mono" value={folder} placeholder="/path/to/project" onChange={(e) => setFolder(e.target.value)} />
          <button className="secondary" onClick={choose}>
            Choose…
          </button>
        </div>
      </div>
      <div className="conversation">
        <p className="muted">
          The session runs in the background as <code>anchorleg run</code>, on the account the policy picks, and keeps going if
          you close the app.
        </p>
      </div>
      <Composer view={null} cwd={folder} onStarted={onStarted} />
    </div>
  );
}

export default function App() {
  const [info, setInfo] = useState<CliInfo | null>(null);
  const [runs, setRuns] = useState<Run[] | null>(null);
  const [requests, setRequests] = useState<Request[]>([]);
  const [page, setPage] = useState<Page>("sessions");
  const [st, setSt] = useState<Status | null>(null);
  const [list, setList] = useState<Account[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const runsRef = useRef<Run[]>([]);
  const lastNotice = useRef<{ run: number; at: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showTerminal, setShowTerminal] = useState(firstLaunch);
  const [selected, setSelected] = useState<number | "new">("new");
  const [folder, setFolderState] = useState(() => remembered(LAST_FOLDER));
  const setFolder = (f: string) => {
    setFolderState(f);
    try {
      localStorage.setItem(LAST_FOLDER, f);
    } catch {
      /* the folder just isn't remembered */
    }
  };

  const refresh = useCallback(async () => {
    const i = await cliInfo();
    setInfo(i);
    if (!i.ok) return;
    try {
      const [rs, reqs] = await Promise.all([sessions(), permissions()]);
      setRuns(rs);
      runsRef.current = rs;
      setRequests(reqs);
      setError(null);
      // A reply sent from elsewhere continues the selected session: follow it.
      setSelected((s) => (s === "new" ? s : newest(rs, s)));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, POLL_MS);
    return () => clearInterval(t);
  }, [refresh]);

  // Notices from runs (D65): the Rust side shows the notification; here, refresh at once and
  // remember the run, so coming back to the window (a click on the notification) opens it.
  useEffect(() => {
    const offNotice = listen<Notice>("notice", (e) => {
      if (e.payload.run_id != null) lastNotice.current = { run: e.payload.run_id, at: Date.now() };
      refresh();
    });
    const offFocus = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      const n = lastNotice.current;
      if (focused && n && Date.now() - n.at < NOTICE_FOCUS_MS) {
        lastNotice.current = null;
        setSelected(newest(runsRef.current, n.run));
      }
    });
    return () => {
      offNotice.then((f) => f());
      offFocus.then((f) => f());
    };
  }, [refresh]);

  const waiting = new Set(requests.map((q) => q.run_id));
  // A request's run, or the newest run continuing it, carries the badge.
  if (runs) for (const q of requests) waiting.add(newest(runs, q.run_id));

  const loadAccounts = useCallback(async (refresh = false) => {
    try {
      const [s, l] = await Promise.all([status(refresh), accounts()]);
      setSt(s);
      setList(l);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    if (!info?.ok) return;
    loadAccounts();
    const t = setInterval(() => loadAccounts(), STATUS_POLL_MS);
    return () => clearInterval(t);
  }, [info?.ok, loadAccounts]);
  const refreshQuota = async () => {
    setRefreshing(true);
    await loadAccounts(true);
    setRefreshing(false);
  };

  // Keep the menu bar item current.
  useEffect(() => {
    if (!st) return;
    const { title, lines } = trayOf(st, requests.length);
    updateTray(title, lines, requests.length).catch(() => {});
  }, [st, requests.length]);

  const started = (run: number) => {
    setSelected(run);
    refresh();
  };

  return (
    <div className="app">
      <header>
        <Logo />
        <h1>anchorleg</h1>
        {info?.ok && (
          <nav className="tabs">
            {PAGES.map(([p, label]) => (
              <button key={p} className={page === p ? "tab active" : "tab"} onClick={() => setPage(p)}>
                {label}
              </button>
            ))}
          </nav>
        )}
        <span className="spacer" />
        {info?.ok && (
          <span className="muted mono">
            CLI {info.version}
            {!info.bundled && " (installed)"}
          </span>
        )}
        {requests.length > 0 && (
          <button
            className="waiting-count"
            onClick={() => {
              setPage("sessions");
              setSelected(newest(runs ?? [], requests[0].run_id));
            }}
          >
            {requests.length} waiting for you
          </button>
        )}
        {info?.bundled && (
          <button className="link" onClick={() => setShowTerminal(true)}>
            Use in Terminal
          </button>
        )}
      </header>
      {!info ? (
        <main>
          <p className="muted">Looking for the anchorleg CLI…</p>
        </main>
      ) : !info.ok ? (
        <main>
          <Setup info={info} />
        </main>
      ) : (
        page === "accounts" ? (
          <main>
            <Accounts status={st} list={list} refreshing={refreshing} onRefresh={refreshQuota} onChanged={() => loadAccounts()} />
          </main>
        ) : page === "usage" ? (
          <main>
            <Usage />
          </main>
        ) : page === "settings" ? (
          <main>
            <Settings onChanged={() => loadAccounts()} />
          </main>
        ) : (
        <div className="body">
          <Sidebar runs={runs} waiting={waiting} error={error} selected={selected} onSelect={setSelected} />
          <div className="main">
            {showTerminal && info.bundled && (
              <div className="pad">
                <TerminalCard onClose={() => setShowTerminal(false)} />
              </div>
            )}
            {selected === "new" ? (
              <NewSession folder={folder} setFolder={setFolder} onStarted={started} />
            ) : (
              <Conversation
                run={selected}
                runs={runs ?? []}
                requests={requests}
                onAnswered={refresh}
                onStarted={started}
                onNew={(cwd) => {
                  setFolder(cwd);
                  setSelected("new");
                }}
              />
            )}
          </div>
        </div>
        )
      )}
    </div>
  );
}
