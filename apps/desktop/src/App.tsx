import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
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
import Settings from "./Settings";
import Usage from "./Usage";
import Limits from "./Limits";
import Conversation, { firstLine } from "./Conversation";
import NewTask from "./NewTask";
import { accountHue, ago, heads, isWorking, nameOf, newest, startedOn } from "./sessions";
import TerminalCard from "./TerminalCard";
import { AppOutdated, isNewerDatabase, TerminalOutdated } from "./VersionCheck";
import { Icon } from "./ui";
import { loadVendors, VendorsContext, type VendorCaps } from "./vendors";

const POLL_MS = 2000;
const TERMINAL_SEEN = "terminal-card-seen";
const LAST_FOLDER = "last-folder";
/** Coming back to the window this soon after a notice opens its session (the notification was clicked). */
const NOTICE_FOCUS_MS = 20_000;
/** Quota readings come from the store (cheap); `status --refresh` asks the CLIs. */
const STATUS_POLL_MS = 15_000;

type Page = "sessions" | "accounts" | "usage" | "settings";
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

function Back({ onBack }: { onBack: () => void }) {
  return (
    <button className="link back" onClick={onBack}>
      <Icon name="back" size={14} /> Tasks
    </button>
  );
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

function Sidebar({
  runs,
  waiting,
  error,
  selected,
  onSelect,
  accounts,
  status: st,
  onImport,
}: {
  runs: Run[] | null;
  /** Opens the Accounts page, where sessions are imported. */
  onImport: () => void;
  /** Account names in the order they're tried: each gets its colour from its place. */
  accounts: string[];
  status: Status | null;
  /** Runs with something waiting for the person. */
  waiting: Set<number>;
  error: string | null;
  selected: number | "new";
  onSelect: (s: number | "new") => void;
}) {
  const [q, setQ] = useState("");
  const list = runs ? heads(runs) : [];
  const shown = q.trim()
    ? list.filter((r) => `${nameOf(runs ?? [], r) ?? ""} ${r.task} ${r.cwd}`.toLowerCase().includes(q.trim().toLowerCase()))
    : list;
  return (
    <nav className="sidebar">
      <div className="sidebar-top">
        <button className={`primary new-task-button ${selected === "new" ? "selected" : ""}`} onClick={() => onSelect("new")}>
          <Icon name="plus" /> New task
        </button>
        {list.length > 5 && (
          <div className="search">
            <Icon name="search" size={13} />
            <input placeholder="Search tasks" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
        )}
      </div>
      {error && <p className="error pad">{error}</p>}
      {!runs && !error && (
        <div className="skeleton-list" aria-label="Loading tasks" role="status">
          {[72, 54, 64, 46].map((w, i) => (
            <div key={i} className="skeleton-row">
              <span className="skeleton" style={{ width: `${w}%` }} />
              <span className="skeleton short" />
            </div>
          ))}
        </div>
      )}
      {runs && list.length === 0 && (
        <p className="muted pad">
          Your tasks show up here.{" "}
          <button className="link" onClick={onImport}>
            Import the sessions you started in Claude Code or Codex
          </button>
        </p>
      )}
      {shown.map((r) => {
        const working = isWorking(r);
        const needs = waiting.has(r.id);
        const [cls, word] = needs
          ? ["waiting", "Needs you"]
          : working
            ? ["running", "Working"]
            : r.state === "failed"
              ? ["failed", "Failed"]
              : r.state === "done"
                ? ["done", "Done"]
                : ["stopped", "Stopped"];
        // The initial of the account the session started on; while it waits for every limit to reset,
        // for the account it will continue on.
        const blocked = r.state === "waiting" && working && st?.next.action === "wait" ? st.next : null;
        const on = blocked ? blocked.account : startedOn(runs ?? [], r);
        const hue = accountHue(accounts, on);
        const resumes = blocked ? new Date(blocked.until * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : null;
        return (
          <button
            key={r.id}
            className={`item ${selected === r.id ? "selected" : ""}`}
            onClick={() => onSelect(r.id)}
            title={`${r.task}\n\n${r.cwd}${on ? `\n${blocked ? `Continues on ${on} at ${resumes}, when limits reset` : `Started on ${on}`}` : ""}`}
          >
            <span className="item-text">
              <span className="item-title">{nameOf(runs ?? [], r) ?? firstLine(r.task)}</span>
              <span className="item-meta muted">
                <span className={`pill pill-${cls} small-pill`}>
                  {working && !needs && <span className="spinner" />} {word}
                </span>{" "}
                {blocked ? `resumes on ${on} at ${resumes}` : `${r.cwd.split("/").filter(Boolean).pop()} · ${ago(r.started_at)} ago`}
              </span>
            </span>
            {hue != null && on && (
              <span className="acct-mark" style={{ "--acct-h": hue } as React.CSSProperties} aria-label={`Account ${on}`}>
                {on.slice(0, 1).toUpperCase()}
              </span>
            )}
          </button>
        );
      })}
    </nav>
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
  // Closed for this launch; it comes back while the data stays newer than the app.
  const [appOutdatedClosed, setAppOutdatedClosed] = useState(false);
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

  const infoRef = useRef<CliInfo | null>(null);
  const refresh = useCallback(async () => {
    // Asked until the CLI is found and new enough; after that it doesn't change.
    const i = infoRef.current?.ok ? infoRef.current : await cliInfo();
    infoRef.current = i;
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

  // The menu bar's Window and Settings items.
  useEffect(() => {
    const off = listen<string>("navigate", (e) => {
      if (e.payload === "new") {
        setPage("sessions");
        setSelected("new");
      } else if (["sessions", "accounts", "usage", "settings"].includes(e.payload)) setPage(e.payload as Page);
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

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

  const [vendorCaps, setVendorCaps] = useState<VendorCaps[]>([]);
  useEffect(() => {
    if (info?.ok) loadVendors().then(setVendorCaps, () => {});
  }, [info?.ok]);

  return (
    <VendorsContext.Provider value={{ vendors: vendorCaps, accounts: list ?? [] }}>
    <div className="app">
      {/* The terminal's anchorleg older than the app's (the card covers it on first launch). */}
      {info?.ok && info.bundled && !showTerminal && !isNewerDatabase(error) && <TerminalOutdated />}
      {isNewerDatabase(error) && !appOutdatedClosed && <AppOutdated onClose={() => setAppOutdatedClosed(true)} />}
      <header>
        <Logo />
        <h1 title={info?.ok ? `CLI ${info.version}${info.bundled ? "" : " (installed)"}` : undefined}>anchorleg</h1>
        <span className="spacer" />
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
        {info?.ok && <Limits st={st} refreshing={refreshing} onRefresh={refreshQuota} onPage={setPage} />}
        {info?.bundled && (
          <button className="icon-text" onClick={() => setShowTerminal(true)} data-tip="Use anchorleg from the Terminal too">
            <Icon name="terminal" size={14} /> Terminal
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
            <Back onBack={() => setPage("sessions")} />
            <Accounts status={st} list={list} refreshing={refreshing} onRefresh={refreshQuota} onChanged={() => loadAccounts()} />
          </main>
        ) : page === "usage" ? (
          <main>
            <Back onBack={() => setPage("sessions")} />
            <Usage />
          </main>
        ) : page === "settings" ? (
          <main>
            <Back onBack={() => setPage("sessions")} />
            <Settings onChanged={() => loadAccounts()} />
          </main>
        ) : (
        <div className="body">
          <Sidebar runs={runs} waiting={waiting} error={error} selected={selected} onSelect={setSelected} accounts={(st?.accounts ?? list ?? []).map((a) => a.name)} status={st} onImport={() => setPage("accounts")} />
          <div className="main">
            {showTerminal && info.bundled && (
              <div className="pad">
                <TerminalCard onClose={() => setShowTerminal(false)} />
              </div>
            )}
            {selected === "new" ? (
              <NewTask folder={folder} setFolder={setFolder} runs={runs ?? []} accounts={list ?? []} status={st} onStarted={started} />
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
    </VendorsContext.Provider>
  );
}
