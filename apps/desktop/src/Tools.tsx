// Beside a session's conversation: what it changed (diff, undo / redo, review) and the side
// questions asked of it (`/btw`).
import { useEffect, useState } from "react";
import { diff, session, start, undo, type Run, type Session } from "./api";
import { question } from "./sessions";

export type ToolTab = "changes" | "side";

export default function Tools({
  view,
  side,
  tab,
  onTab,
  onClose,
  onStarted,
}: {
  view: Session;
  /** Side questions asked of this session, newest first. */
  side: Run[];
  tab: ToolTab;
  onTab: (t: ToolTab) => void;
  onClose: () => void;
  onStarted: (run: number) => void;
}) {
  return (
    <aside className="tools">
      <div className="tools-head">
        <button className={tab === "changes" ? "tab active" : "tab"} onClick={() => onTab("changes")}>
          Changed files
        </button>
        <button className={tab === "side" ? "tab active" : "tab"} onClick={() => onTab("side")}>
          Side questions{side.length ? ` (${side.length})` : ""}
        </button>
        <span className="spacer" />
        <button className="link" onClick={onClose} aria-label="Close">
          ×
        </button>
      </div>
      {tab === "changes" ? <Changes view={view} onStarted={onStarted} /> : <Side side={side} />}
    </aside>
  );
}

function Changes({ view, onStarted }: { view: Session; onStarted: (run: number) => void }) {
  const run = view.run.id;
  const [text, setText] = useState<string | null>(null);
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
    load();
  }, [run]);
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
        <button className="secondary small" disabled={busy} onClick={load}>
          Refresh
        </button>
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
      {text != null && (text.trim() === "" || text.startsWith("no changes") ? <p className="muted">No files have changed since the task started.</p> : <Diff text={text} />)}
    </div>
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
