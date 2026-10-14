import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { rename, session, stop, type Message, type Request, type Run, type Session } from "./api";
import Composer from "./Composer";
import Requests from "./Requests";
import { sideRuns } from "./sessions";
import Tools, { type ToolTab } from "./Tools";
import { short, tilde } from "./sessions";

const POLL_WORKING_MS = 1000;
const POLL_IDLE_MS = 4000;

/** One session, live: `anchorleg sessions show <run> --since N --json`, polled. */
export default function Conversation({
  run,
  runs,
  requests,
  onAnswered,
  onStarted,
  onNew,
}: {
  run: number;
  /** Every listed run, for the side questions asked of this session. */
  runs: Run[];
  /** Everything waiting for the person; this session's are shown. */
  requests: Request[];
  onAnswered: () => void;
  /** A run this view started (a reply or a fork): select it. */
  onStarted: (run: number) => void;
  /** `/clear`: a new session in this folder. */
  onNew: (cwd: string) => void;
}) {
  const [view, setView] = useState<Session | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tools, setTools] = useState<ToolTab | null>(null);
  // Text put in the message box from elsewhere (a click on a run: `/rewind N `).
  const [seed, setSeed] = useState<{ text: string; key: number } | null>(null);
  // The run the messages are for and how far they go; a reply's run shows the same history.
  const copy = useRef<{ run: number; messages: Message[] }>({ run, messages: [] });

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      const mine = copy.current.run === run ? copy.current.messages : [];
      try {
        // The last message can still grow while the agent works: ask for it again.
        const s = await session(run, Math.max(0, mine.length - 1));
        if (!alive) return;
        const messages = mine.slice(0, s.since).concat(s.messages);
        copy.current = { run, messages };
        setView({ ...s, messages });
        setError(null);
        timer = setTimeout(poll, s.working ? POLL_WORKING_MS : POLL_IDLE_MS);
      } catch (e) {
        if (!alive) return;
        setError(String(e));
        timer = setTimeout(poll, POLL_IDLE_MS);
      }
    };
    poll();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [run]);

  // Follow the bottom unless the person scrolled up.
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [view]);
  const onScroll = () => {
    const el = scroller.current;
    if (el) pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };

  const mine = view ? requests.filter((q) => view.runs.includes(q.run_id)) : [];
  // A new request scrolls into view.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && mine.length) el.scrollTop = el.scrollHeight;
  }, [mine.length]);

  if (!view) return <div className="pane">{error ? <p className="error pad">{error}</p> : <p className="muted pad">Loading…</p>}</div>;
  const r = view.run;
  const side = sideRuns(runs, view.runs);
  return (
    <div className="pane-row">
    <div className="pane">
      <Head
        view={view}
        side={side.length}
        onTools={(t) => setTools(tools === t ? null : t)}
        onRewind={(id) => setSeed({ text: `/rewind ${id} `, key: Date.now() })}
      />
      <div className="conversation" ref={scroller} onScroll={onScroll}>
        {view.messages.map((m, i) => (
          <Line key={i} m={m} />
        ))}
        <Requests requests={mine} onAnswered={onAnswered} />
        {view.working && mine.length === 0 && (
          <div className="working">
            <span className="spinner" /> {view.running ? <span className="mono">{view.running.what}</span> : "working…"}
            {view.running?.output && <pre className="tail">{view.running.output}</pre>}
          </div>
        )}
        {!view.working && r.state === "failed" && <p className="error">Run #{r.id} failed.</p>}
      </div>
      {(view.steps.length > 0 || view.subagents.length > 0) && <Progress view={view} />}
      {error && <p className="error pad">{error}</p>}
      <Composer
        view={view}
        seed={seed}
        onStarted={onStarted}
        onNew={onNew}
        onSide={() => setTools("side")}
      />
    </div>
    {tools && <Tools view={view} side={side} tab={tools} onTab={setTools} onClose={() => setTools(null)} onStarted={onStarted} />}
    </div>
  );
}

function Head({
  view,
  side,
  onTools,
  onRewind,
}: {
  view: Session;
  side: number;
  onTools: (t: ToolTab) => void;
  /** Go back to before this run: puts `/rewind N ` in the message box. */
  onRewind: (run: number) => void;
}) {
  const r = view.run;
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setEditing(false);
    try {
      await rename(r.id, name.trim());
    } catch (e) {
      setError(String(e));
    }
  };
  const ctx = view.context;
  return (
    <div className="pane-head">
      <div className="title">
        {editing ? (
          <input
            autoFocus
            value={name}
            placeholder="name this session"
            onChange={(e) => setName(e.target.value)}
            onBlur={save}
            onKeyDown={(e) => {
              if (e.key === "Enter") save();
              if (e.key === "Escape") setEditing(false);
            }}
          />
        ) : (
          <h2
            title="Rename"
            onClick={() => {
              setName(view.name ?? "");
              setEditing(true);
            }}
          >
            {view.name ?? `#${r.id}`}
          </h2>
        )}
        <span className={`state state-${view.working ? "running" : r.state}`}>{view.working ? r.state : r.state === "running" ? "ended" : r.state}</span>
        {view.working && (
          <button className="danger small" onClick={() => stop(r.id).catch((e) => setError(String(e)))}>
            Stop
          </button>
        )}
        <span className="spacer" />
        <button className="link" onClick={() => onTools("changes")} title="What the session changed: diff, undo, review">
          Changes
        </button>
        <button className="link" onClick={() => onTools("side")} title="Side questions (/btw)">
          Side questions{side ? ` (${side})` : ""}
        </button>
      </div>
      <div className="meta muted mono">
        <span title={r.cwd}>{tilde(r.cwd)}</span>
        {r.account && <span>{r.account}</span>}
        {r.mode && <span>{r.mode}</span>}
        <span className="runs-chain">
          {view.runs.map((id, i) => (
            <span key={id}>
              {i > 0 && " → "}
              <button className="link mono small" title={`Go back to before #${id} (/rewind)`} onClick={() => onRewind(id)}>
                #{id}
              </button>
            </span>
          ))}
        </span>
        {ctx.tokens != null && (
          <span title="context used">
            ctx {short(ctx.tokens)}
            {ctx.window ? `/${short(ctx.window)} (${Math.round((ctx.tokens * 100) / ctx.window)}%)` : ""}
          </span>
        )}
      </div>
      {r.goal && (
        <div className="meta muted">
          goal: {r.goal} {r.goal_status && <span className="mono">({r.goal_status})</span>}
        </div>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  );
}

function Line({ m }: { m: Message }) {
  switch (m.role) {
    case "user":
      return <div className="msg user">{m.text}</div>;
    case "agent":
      return <div className="msg agent">{m.text}</div>;
    case "tool":
      return (
        <div className="msg tool mono">
          <span className="dot">⏺</span> {m.text}
        </div>
      );
    case "output":
      return <pre className="msg output">{m.text}</pre>;
    case "tool_error":
      return <pre className="msg output error">{m.text}</pre>;
    case "thinking":
      return <div className="msg thinking">{m.text}</div>;
    case "anchorleg":
      return <div className="msg note">anchorleg · {m.text}</div>;
    case "error":
      return <div className="msg error">{m.text}</div>;
    default:
      return <div className="msg">{m.text}</div>;
  }
}

function Progress({ view }: { view: Session }) {
  return (
    <div className="progress">
      {view.steps.length > 0 && (
        <ul className="steps">
          {view.steps.map((s, i) => (
            <li key={i} className={`step-${s.status}`}>
              {s.status === "done" ? "✓" : s.status === "in_progress" ? "▸" : "○"} {s.text}
            </li>
          ))}
        </ul>
      )}
      {view.subagents.length > 0 && (
        <ul className="subagents">
          {view.subagents.map((a) => (
            <li key={a.id}>
              {a.done ? "✓" : <span className="spinner" />} {a.name}{" "}
              <span className="muted mono">
                {a.steps} steps{!a.done && a.doing ? ` · ${a.doing}` : ""}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
