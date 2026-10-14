import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { describeOptions, session, type Message, type Request, type Run, type Session } from "./api";
import Composer from "./Composer";
import Requests from "./Requests";
import SessionActions, { ForkDialog, RewindDialog } from "./SessionActions";
import { copyText } from "./share";
import { short, sideRuns, tilde } from "./sessions";
import Tools, { type ToolTab } from "./Tools";
import { APPROVAL, Icon } from "./ui";
import { useVendors } from "./vendors";

const POLL_WORKING_MS = 1000;
const POLL_IDLE_MS = 4000;

/** One task, live: `anchorleg sessions show <run> --since N --json`, polled. */
export default function Conversation({
  run,
  runs,
  requests,
  onAnswered,
  onStarted,
  onNew,
}: {
  run: number;
  /** Every listed run, for the side questions asked of this task. */
  runs: Run[];
  /** Everything waiting for the person; this task's are shown. */
  requests: Request[];
  onAnswered: () => void;
  /** A run this view started (a reply, a fork, a review): select it. */
  onStarted: (run: number) => void;
  /** A new task in this folder. */
  onNew: (cwd: string) => void;
}) {
  const [view, setView] = useState<Session | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tools, setTools] = useState<ToolTab | null>(null);
  // The approval setting picked for the next message (null: the task's own).
  const [mode, setMode] = useState<string | null>(null);
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
        <div className="pane-head">
          <div className="title">
            <h2 title={r.task}>{view.name ?? firstLine(r.task)}</h2>
            <Status view={view} waiting={mine.length > 0} />
            <div className="meta">
              <span className="meta-icon" role="img" tabIndex={0} aria-label={tilde(r.cwd)} data-tip={tilde(r.cwd)}>
                <Icon name="folder" size={15} />
              </span>
              {r.account && (
                <span className="meta-icon" role="img" tabIndex={0} aria-label={`Running on ${r.account}`} data-tip={`Running on ${r.account}`}>
                  <Icon name="user" size={15} />
                </span>
              )}
              {(mode ?? r.mode) && (
                <span className="meta-icon" role="img" tabIndex={0} aria-label={`Approvals: ${APPROVAL.find((a) => a.value === (mode ?? r.mode))?.label}`} data-tip={`Approvals: ${APPROVAL.find((a) => a.value === (mode ?? r.mode))?.label}`}>
                  <Icon name="shield" size={15} />
                </span>
              )}
              {view.context.tokens != null && view.context.window ? (
                <span
                  className="meta-icon"
                  role="img" tabIndex={0} aria-label={`Memory ${Math.round((view.context.tokens * 100) / view.context.window)}% used (${short(view.context.tokens)} of ${short(view.context.window)} tokens)`}
                  data-tip={`Memory ${Math.round((view.context.tokens * 100) / view.context.window)}% used (${short(view.context.tokens)} of ${short(view.context.window)} tokens)`}
                >
                  <Icon name="gauge" size={15} />
                </span>
              ) : null}
              {describeOptions(r.options, tilde).length > 0 && (
                <span className="meta-icon" role="img" tabIndex={0} aria-label={describeOptions(r.options, tilde).join(". ")} data-tip={describeOptions(r.options, tilde).join("\n")}>
                  <Icon name="sliders" size={15} />
                </span>
              )}
              {r.goal && (
                <span
                  className={`meta-icon ${r.goal_status === "met" ? "ok" : ""}`}
                  role="img" tabIndex={0} aria-label={`Keep going until “${r.goal}”${r.goal_status === "met" ? ": met" : r.goal_status === "unmet" ? ": not met yet" : ""}`}
                  data-tip={`Keep going until “${r.goal}”${r.goal_status === "met" ? ": met" : r.goal_status === "unmet" ? ": not met yet" : ""}`}
                >
                  <Icon name="goal" size={15} />
                </span>
              )}
            </div>
            <span className="spacer" />
            <SessionActions view={view} runs={runs} mode={mode} setMode={setMode} onTools={(t) => setTools(tools === t ? null : t)} onStarted={onStarted} onNew={onNew} />
          </div>
        </div>
        <div className="conversation" ref={scroller} onScroll={onScroll}>
          <Lines view={view} runs={runs} onStarted={onStarted} />
          <Requests requests={mine} onAnswered={onAnswered} />
          {view.working && mine.length === 0 && (
            <div className="working">
              <span className="spinner" /> {view.running ? friendly(view.running.what) : "Working…"}
              {view.running?.output && <pre className="tail">{view.running.output}</pre>}
            </div>
          )}
          {!view.working && mine.length === 0 && r.state === "failed" && <p className="error">This step failed. You can reply to try again or say what to do differently.</p>}
        </div>
        {(view.steps.length > 0 || view.subagents.length > 0) && <Progress view={view} />}
        {error && <p className="error pad">{error}</p>}
        <Composer view={view} mode={mode} onSent={() => setMode(null)} onStarted={onStarted} onNew={onNew} onSide={() => setTools("side")} />
      </div>
      {tools && <Tools view={view} side={side} tab={tools} onTab={setTools} onClose={() => setTools(null)} onStarted={onStarted} />}
    </div>
  );
}

export const firstLine = (s: string) => s.trim().split("\n")[0].slice(0, 80) || "Untitled task";

function Status({ view, waiting }: { view: Session; waiting: boolean }) {
  const r = view.run;
  const [cls, word] = waiting
    ? ["waiting", "Needs your answer"]
    : view.working
      ? ["running", r.state === "switching" ? "Switching account…" : "Working…"]
      : r.state === "failed"
        ? ["failed", "Failed"]
        : r.state === "stopped" || r.state === "running" || r.state === "waiting"
          ? ["stopped", "Stopped"]
          : ["done", "Done"];
  return <span className={`pill pill-${cls}`}>{word}</span>;
}

/** `Bash  cargo test` → "Ran cargo test", in words. */
export function friendly(what: string): string {
  const [tool, ...rest] = what.split("  ");
  const detail = rest.join("  ").trim();
  const verb: Record<string, string> = {
    Bash: "Ran",
    Read: "Read",
    Write: "Created",
    Edit: "Changed",
    MultiEdit: "Changed",
    NotebookEdit: "Changed",
    Grep: "Searched for",
    Glob: "Looked for files",
    LS: "Listed",
    WebFetch: "Opened",
    WebSearch: "Searched the web for",
    Task: "Asked a helper to",
    Agent: "Asked a helper to",
    TodoWrite: "Updated its task list",
  };
  const v = verb[tool];
  if (v) return detail ? `${v} ${detail}` : v;
  if (tool.startsWith("mcp__")) return `Used ${tool.split("__").slice(1).join(" ")}${detail ? `: ${detail}` : ""}`;
  return what;
}

/** A note from anchorleg, or a hook's output: shown only when asked for. */
const isNote = (m: Message) => m.role === "anchorleg" || (m.role === "thinking" && m.text.startsWith("hook "));

type MsgDialog = { kind: "rewind"; to: number; message: string; title: string } | { kind: "fork" } | null;

/** Tool calls are grouped with their output, which opens on a click; notes in a row fold into one line. */
function Lines({ view, runs, onStarted }: { view: Session; runs: Run[]; onStarted: (run: number) => void }) {
  const [dialog, setDialog] = useState<MsgDialog>(null);
  const caps = useVendors().ofAccount(view.run.account);
  const messages = view.messages;
  // Each run starts with the message that was sent: match them in order, for "go back to here".
  const runOf = new Map<number, number>();
  const chain = view.runs.map((id) => runs.find((r) => r.id === id));
  let j = 0;
  messages.forEach((m, i) => {
    if (m.role !== "user") return;
    for (let k = j; k < chain.length; k++)
      if (chain[k]?.task.trim() === m.text.trim()) {
        runOf.set(i, chain[k]!.id);
        j = k + 1;
        break;
      }
  });
  let lastAgent = -1;
  messages.forEach((m, i) => m.role === "agent" && (lastAgent = i));
  const out: JSX.Element[] = [];
  for (let i = 0; i < messages.length; i++) {
    const m = messages[i];
    if (isNote(m)) {
      const notes = [m];
      while (i + 1 < messages.length && isNote(messages[i + 1])) notes.push(messages[++i]);
      out.push(<Notes key={i} notes={notes} />);
    } else if (m.role === "tool") {
      const result = messages[i + 1]?.role === "output" || messages[i + 1]?.role === "tool_error" ? messages[++i] : undefined;
      out.push(<Step key={i} what={m.text} result={result} />);
    } else if (m.role === "user") {
      const run = runOf.get(i);
      const canRewind = run != null && run !== view.runs[0] && !view.working && (caps?.rewind ?? true);
      out.push(
        <div key={i} className="msg-row user">
          <MsgActions
            text={m.text}
            extra={
              canRewind
                ? [
                    { icon: "pencil", tip: "Edit and send again", onClick: () => setDialog({ kind: "rewind", to: run, message: m.text, title: "Edit and send again" }) },
                    { icon: "back", tip: "Go back to before this message", onClick: () => setDialog({ kind: "rewind", to: run, message: "", title: "Go back to before this message" }) },
                  ]
                : []
            }
          />
          <div className="msg user">{m.text}</div>
        </div>,
      );
    } else if (m.role === "agent") {
      out.push(
        <div key={i} className="msg-row agent">
          <span className="avatar" aria-hidden="true">
            <Icon name="sparkle" size={13} />
          </span>
          <div className="msg agent">{m.text}</div>
          <MsgActions
            text={m.text}
            extra={i === lastAgent && (caps?.fork ?? true) ? [{ icon: "fork", tip: "Try another direction from here", onClick: () => setDialog({ kind: "fork" }) }] : []}
          />
        </div>,
      );
    } else out.push(<Line key={i} m={m} />);
  }
  return (
    <>
      {out}
      {dialog?.kind === "rewind" && (
        <RewindDialog view={view} runs={runs} onClose={() => setDialog(null)} onStarted={onStarted} initial={{ to: dialog.to, message: dialog.message, title: dialog.title }} />
      )}
      {dialog?.kind === "fork" && <ForkDialog view={view} onClose={() => setDialog(null)} onStarted={onStarted} />}
    </>
  );
}

type Extra = { icon: string; tip: string; onClick: () => void };

/** Shown on hover beside a message: copy it, and what else fits. */
function MsgActions({ text, extra }: { text: string; extra: Extra[] }) {
  const [copied, setCopied] = useState(false);
  const copy = () =>
    copyText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  return (
    <div className="msg-actions">
      <button className="icon-button" onClick={copy} aria-label="Copy" data-tip={copied ? "Copied" : "Copy"}>
        <Icon name={copied ? "check" : "copy"} size={14} />
      </button>
      {extra.map((x) => (
        <button key={x.tip} className="icon-button" onClick={x.onClick} aria-label={x.tip} data-tip={x.tip}>
          <Icon name={x.icon} size={14} />
        </button>
      ))}
    </div>
  );
}

function Notes({ notes }: { notes: Message[] }) {
  const [open, setOpen] = useState(false);
  const text = (m: Message) => (m.role === "thinking" ? m.text.replace(/^hook /, "Hook: ") : m.text);
  return (
    <div className={`notes ${open ? "open" : ""}`}>
      <button className="notes-toggle" onClick={() => setOpen(!open)} aria-expanded={open}>
        <Icon name="info" size={13} />
        {notes.length === 1 ? "1 update" : `${notes.length} updates`}
        <Icon name="chevron" size={12} />
      </button>
      {open && (
        <ul className="notes-list">
          {notes.map((n, k) => (
            <li key={k}>{text(n)}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Step({ what, result }: { what: string; result?: Message }) {
  const [open, setOpen] = useState(false);
  const failed = result?.role === "tool_error";
  return (
    <div className={`step ${failed ? "failed" : ""}`}>
      <button className="step-line" onClick={() => result && setOpen(!open)} disabled={!result}>
        <span className="dot">{failed ? "!" : "•"}</span> {friendly(what)}
        {result && <span className="step-more">{open ? "Hide" : failed ? "Show the error" : "Show output"}</span>}
      </button>
      {open && result && <pre className={`msg output ${failed ? "error" : ""}`}>{result.text}</pre>}
    </div>
  );
}

function Line({ m }: { m: Message }) {
  const [open, setOpen] = useState(false);
  switch (m.role) {
    case "output":
      return <pre className="msg output">{m.text}</pre>;
    case "tool_error":
      return <pre className="msg output error">{m.text}</pre>;
    case "thinking":
      return (
        <button className="msg thinking" onClick={() => setOpen(!open)}>
          {open ? m.text : "Thought it over… (show)"}
        </button>
      );
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
        <>
          <div className="progress-title">Plan</div>
          <ul className="steps">
            {view.steps.map((s, i) => (
              <li key={i} className={`step-${s.status}`}>
                {s.status === "done" ? "✓" : s.status === "in_progress" ? "▸" : "○"} {s.text}
              </li>
            ))}
          </ul>
        </>
      )}
      {view.subagents.length > 0 && (
        <>
          <div className="progress-title">Helpers</div>
          <ul className="subagents">
            {view.subagents.map((a) => (
              <li key={a.id}>
                {a.done ? "✓" : <span className="spinner" />} {a.name}{" "}
                <span className="muted small">
                  {a.steps} steps{!a.done && a.doing ? ` · ${friendly(a.doing)}` : ""}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
