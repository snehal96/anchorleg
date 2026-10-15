// Beside a session's conversation: what it changed (diff, undo / redo, review), the side
// questions asked of it (`/btw`) and its subagents.
import { useEffect, useState } from "react";
import { agents as loadAgents, diff, openTarget, session, start, undo, type FileChange, type Message, type Run, type Session, type SubAgent } from "./api";
import { Step } from "./Conversation";
import { Markdown } from "./Markdown";
import { ago, question, tilde } from "./sessions";
import { Icon } from "./ui";

export type ToolTab = "changes" | "side" | "agents";

export default function Tools({
  view,
  side,
  tab,
  onTab,
  onClose,
  onStarted,
  onAsk,
}: {
  view: Session;
  /** Side questions asked of this session, newest first. */
  side: Run[];
  tab: ToolTab;
  onTab: (t: ToolTab) => void;
  onClose: () => void;
  onStarted: (run: number) => void;
  /** Put this in the reply box (a question to the main agent about a subagent's work). */
  onAsk: (text: string) => void;
}) {
  return (
    <aside className="tools">
      <div className="tools-head">
        <button className={tab === "changes" ? "tab active" : "tab"} onClick={() => onTab("changes")}>
          Changed files{view.changes?.length ? ` (${view.changes.length})` : ""}
        </button>
        <button className={tab === "side" ? "tab active" : "tab"} onClick={() => onTab("side")}>
          Side questions{side.length ? ` (${side.length})` : ""}
        </button>
        <button className={tab === "agents" ? "tab active" : "tab"} onClick={() => onTab("agents")}>
          Sub-agents
        </button>
        <span className="spacer" />
        <button className="link" onClick={onClose} aria-label="Close">
          ×
        </button>
      </div>
      {tab === "changes" ? <Changes view={view} onStarted={onStarted} /> : tab === "side" ? <Side side={side} /> : <Agents view={view} onAsk={onAsk} />}
    </aside>
  );
}

function Changes({ view, onStarted }: { view: Session; onStarted: (run: number) => void }) {
  const run = view.run.id;
  const [text, setText] = useState<string | null>(null);
  // The files the CLIs reported, or the whole folder compared with git (catches shell edits).
  const [whole, setWhole] = useState(false);
  const reported = view.changes;
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<string | null | undefined>(undefined);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const load = () =>
    diff(run).then(
      (d) => (setText(d), setError(null)),
      (e) => (setText(null), setError(String(e))),
    );
  useEffect(() => {
    setText(null);
    setPreview(undefined);
    setNote(null);
    // An older CLI sends no `changes`: compare the folder instead.
    if (whole || reported === undefined) load();
  }, [run, whole]);
  const act = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const working = view.working;
  return (
    <div className="tools-body">
      <div className="tools-actions">
        {reported !== undefined && (
          <label className="check small" title="Compare the whole folder with how the task found it (git); catches files changed by commands too">
            <input type="checkbox" checked={whole} onChange={(e) => setWhole(e.target.checked)} /> Whole folder
          </label>
        )}
        {(whole || reported === undefined) && (
          <button className="secondary small" disabled={busy} onClick={load}>
            Refresh
          </button>
        )}
        <button
          className="secondary small"
          disabled={busy || working}
          title={working ? "Stop the task first" : "Put the files back as they were when the task started"}
          onClick={() => act(async () => setPreview((await undo(run, "preview")).preview ?? null))}
        >
          Undo changes…
        </button>
        <button className="secondary small" disabled={busy || working} title="Bring back what the last undo put back" onClick={() => act(async () => (setNote((await undo(run, "redo")).done ?? null), await load()))}>
          Bring changes back
        </button>
        <span className="spacer" />
        <button
          className="secondary small"
          disabled={busy || working}
          title="Another account looks over these changes without editing anything"
          onClick={() => act(async () => onStarted((await start({ text: "", review: run })).run_id))}
        >
          Second opinion
        </button>
        <button
          className="secondary small"
          disabled={busy || working}
          title="Get a second opinion, then have this task fix what it finds"
          onClick={() => act(async () => onStarted((await start({ text: "", review: run, fix: true })).run_id))}
        >
          Second opinion &amp; fix
        </button>
      </div>
      {preview !== undefined && (
        <div className="request">
          {preview == null ? (
            <p className="muted">No files have changed since the task started.</p>
          ) : (
            <>
              <div className="request-title">Put these files back as they were when the task started?</div>
              <pre className="diff">{preview}</pre>
              <p className="muted small">A backup is kept: “Bring changes back” restores them. The agent isn't told; say so in your next message.</p>
              <div className="request-actions">
                <button className="danger" disabled={busy} onClick={() => act(async () => (setNote((await undo(run, "undo")).done ?? null), setPreview(undefined), await load()))}>
                  Undo the changes
                </button>
                <button className="secondary" onClick={() => setPreview(undefined)}>
                  Keep them
                </button>
              </div>
            </>
          )}
        </div>
      )}
      {note && <p className="ok small">{note}</p>}
      {error && <p className="error small">{error}</p>}
      {reported !== undefined && !whole ? (
        <Reported files={reported} cwd={view.run.cwd} />
      ) : (
        text != null && (text.trim() === "" || text.startsWith("no changes") ? <p className="muted">No files have changed since the task started.</p> : <Diff text={text} />)
      )}
    </div>
  );
}

/** The files the session changed, as its CLIs reported them; the selected one's diff below. */
function Reported({ files, cwd }: { files: FileChange[]; cwd: string }) {
  const [open, setOpen] = useState<string | null>(null);
  if (files.length === 0) return <p className="muted">No files changed yet.</p>;
  const sel = files.find((f) => f.path === open) ?? files[0];
  const rel = (p: string) => (p.startsWith(cwd + "/") ? p.slice(cwd.length + 1) : tilde(p));
  return (
    <>
      <ul className="changed-files">
        {files.map((f) => (
          <li key={f.path}>
            <button className={`changed-file ${f === sel ? "selected" : ""}`} onClick={() => setOpen(f.path)} title={f.path}>
              <span className={`change-kind kind-${f.kind}`}>{f.kind === "created" ? "new" : f.kind === "deleted" ? "gone" : "edit"}</span>
              <span className="mono grow ellipsis">{rel(f.path)}</span>
              {f.has_diff && (
                <span className="mono small">
                  <span className="add-n">+{f.added}</span> <span className="del-n">−{f.removed}</span>
                </span>
              )}
            </button>
          </li>
        ))}
      </ul>
      {sel.kind !== "deleted" && (
        <div className="tools-actions">
          <span className="mono small muted ellipsis grow">{rel(sel.path)}</span>
          <button className="secondary small" onClick={() => openTarget(sel.path).catch(() => {})} title="Open it in the app your system uses for it">
            Open
          </button>
        </div>
      )}
      {sel.has_diff ? <Diff text={sel.diff} /> : <p className="muted small">The agent named this file but didn't send what changed (Antigravity). Turn on “Whole folder” to compare it.</p>}
    </>
  );
}

/** A unified diff, coloured; one block per file. */
function Diff({ text }: { text: string }) {
  return (
    <pre className="diff full">
      {text.split("\n").map((l, i) => (
        <div
          key={i}
          className={
            l.startsWith("diff --git") ? "file" : l.startsWith("+++") || l.startsWith("---") ? "ctx" : l.startsWith("+") ? "add" : l.startsWith("-") ? "del" : l.startsWith("@@") ? "hunk" : "ctx"
          }
        >
          {l || " "}
        </div>
      ))}
    </pre>
  );
}

function Side({ side }: { side: Run[] }) {
  if (side.length === 0)
    return (
      <div className="tools-body">
        <p className="muted">
          Ask with <code>/btw &lt;question&gt;</code>: a read-only copy of the session answers, and the session itself goes on untouched.
        </p>
      </div>
    );
  return (
    <div className="tools-body">
      {side.map((r) => (
        <SideAnswer key={r.id} r={r} />
      ))}
    </div>
  );
}

function SideAnswer({ r }: { r: Run }) {
  const [answer, setAnswer] = useState<string | null>(null);
  const [working, setWorking] = useState(true);
  useEffect(() => {
    let alive = true;
    let t: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const s = await session(r.id);
        if (!alive) return;
        // The copy's answer: its last message, after the question.
        const lastAsk = s.messages.map((m) => m.role).lastIndexOf("user");
        const mine = s.messages.slice(lastAsk + 1);
        setAnswer(mine.filter((m) => m.role === "agent").map((m) => m.text).join("\n\n") || null);
        setWorking(s.working);
        if (s.working) t = setTimeout(poll, 1500);
      } catch {
        if (alive) t = setTimeout(poll, 4000);
      }
    };
    poll();
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [r.id]);
  return (
    <div className="side-answer">
      <div className="msg user">{question(r.task)}</div>
      {answer ? <div className="msg agent">{answer}</div> : <p className="muted small">{working ? "answering…" : "no answer"}</p>}
    </div>
  );
}

/** The agents this session's agent started to work on parts of the task, working or done:
 * what each was asked, its steps and what it reported (`anchorleg sessions agents`). They can't
 * be messaged; the main agent can be asked about one. */
function Agents({ view, onAsk }: { view: Session; onAsk: (text: string) => void }) {
  const [list, setList] = useState<SubAgent[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const run = view.run.id;
  const live = view.working || view.subagents.some((a) => !a.done);
  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    const load = () =>
      loadAgents(run).then(
        (a) => {
          if (!alive) return;
          setList(a);
          setError(null);
          if (live) timer = setTimeout(load, 3000);
        },
        (e) => alive && setError(String(e)),
      );
    load();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [run, live]);
  if (error) return <p className="error pad">{error}</p>;
  if (!list)
    return (
      <div className="tools-body">
        <div className="skeleton-list">
          {[60, 45].map((w, i) => (
            <div key={i} className="skeleton-row">
              <span className="skeleton" style={{ width: `${w}%` }} />
              <span className="skeleton short" />
            </div>
          ))}
        </div>
      </div>
    );
  if (list.length === 0)
    return (
      <div className="tools-body">
        <p className="muted small">
          No sub-agents yet. When the agent hands parts of the task to agents of its own (Claude Code's Agent tool, Codex's spawned agents), they show here with what each was asked and found.
        </p>
      </div>
    );
  const working = list.filter((a) => a.status === "working").length;
  return (
    <div className="tools-body">
      <p className="muted small agents-summary">
        {list.length} sub-agent{list.length === 1 ? "" : "s"}
        {working ? `, ${working} working` : ""}. They can't be messaged; ask the main agent about one instead.
      </p>
      <ul className="agents">
        {[...list].reverse().map((a) => (
          <AgentRow key={a.id} a={a} open={open === a.id} onToggle={() => setOpen(open === a.id ? null : a.id)} cwd={view.run.cwd} onAsk={onAsk} />
        ))}
      </ul>
    </div>
  );
}

function AgentRow({ a, open, onToggle, cwd, onAsk }: { a: SubAgent; open: boolean; onToggle: () => void; cwd: string; onAsk: (text: string) => void }) {
  const [fullPrompt, setFullPrompt] = useState(false);
  const steps: { what: string; result?: Message }[] = [];
  for (let i = 0; i < a.messages.length; i++) {
    const m = a.messages[i];
    if (m.role !== "tool") continue;
    const next = a.messages[i + 1];
    steps.push({ what: m.text, result: next && (next.role === "output" || next.role === "tool_error") ? next : undefined });
  }
  const ask = () => {
    const report = (a.result ?? "").trim();
    const quoted = report
      ? report
          .slice(0, 600)
          .split("\n")
          .map((l) => `> ${l}`)
          .join("\n") + (report.length > 600 ? "\n> …" : "")
      : "> (it hasn't reported back yet)";
    onAsk(`About the sub-agent “${a.name}”, which reported:\n${quoted}\n\n`);
  };
  return (
    <li className={`agent ${open ? "open" : ""}`}>
      <button className="agent-line" onClick={onToggle} aria-expanded={open}>
        <span className={`agent-mark ${a.status}`}>{a.status === "working" ? <span className="spinner" /> : a.status === "done" ? <Icon name="check" size={13} /> : "·"}</span>
        <span className="agent-name">{a.name}</span>
        <span className="agent-meta muted">
          {a.kind ? `${a.kind} · ` : ""}
          {a.steps} step{a.steps === 1 ? "" : "s"} · {ago(a.updated_at)} ago
        </span>
        <Icon name="chevron" size={12} />
      </button>
      {open && (
        <div className="agent-detail">
          <div className="agent-label">Asked to</div>
          <div className={`agent-prompt ${fullPrompt ? "" : "clamped"}`}>
            <Markdown source={a.prompt || "(no task recorded)"} cwd={cwd} />
          </div>
          {a.prompt.split("\n").length > 6 || a.prompt.length > 500 ? (
            <button className="link small" onClick={() => setFullPrompt(!fullPrompt)}>
              {fullPrompt ? "Show less" : "Show all"}
            </button>
          ) : null}
          {steps.length > 0 && (
            <>
              <div className="agent-label">Steps</div>
              <div className="agent-steps">
                {steps.map((s, i) => (
                  <Step key={i} what={s.what} result={s.result} />
                ))}
              </div>
            </>
          )}
          <div className="agent-label">{a.status === "done" ? "Reported" : a.status === "working" ? "Still working" : "Stopped without a report"}</div>
          {a.result && (
            <div className="agent-result">
              <Markdown source={a.result} cwd={cwd} />
            </div>
          )}
          <div className="agent-actions">
            <button className="secondary small" onClick={ask}>
              <Icon name="chat" size={13} /> Ask the main agent about this
            </button>
          </div>
        </div>
      )}
    </li>
  );
}
