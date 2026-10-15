// What a session waits for, in its conversation: a tool call to approve, questions from the
// agent, or a plan. Answers go through `anchorleg permission answer` (docs/CLI.md).
import { useEffect, useState } from "react";
import { answer, approvePlan, openTarget, type Question, type Request } from "./api";
import { Markdown } from "./Markdown";

/** Text with its web links clickable (an MCP server's "open this page, then answer Done"). */
function Linked({ text }: { text: string }) {
  const parts = text.split(/(https?:\/\/[^\s<>"']+[^\s<>"'.,;:!?)])/);
  return (
    <>
      {parts.map((p, i) =>
        i % 2 ? (
          <a key={i} href={p} onClick={(e) => (e.preventDefault(), openTarget(p))}>
            {p}
          </a>
        ) : (
          p
        ),
      )}
    </>
  );
}

export default function Requests({ requests, onAnswered }: { requests: Request[]; onAnswered: () => void }) {
  return (
    <>
      {requests.map((r) => (
        <RequestCard key={r.id} r={r} onAnswered={onAnswered} />
      ))}
    </>
  );
}

function RequestCard({ r, onAnswered }: { r: Request; onAnswered: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
      onAnswered();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  return (
    <div className={`request request-${r.kind}`}>
      {r.kind === "tool" && <ToolCall r={r} busy={busy} run={run} />}
      {r.kind === "question" && <Questions r={r} busy={busy} run={run} />}
      {r.kind === "plan" && <Plan r={r} busy={busy} run={run} />}
      <div className="request-foot">
        {error && <span className="error">{error}</span>}
        <span className="spacer" />
        <Countdown deadline={r.deadline} />
      </div>
    </div>
  );
}

type Act = { r: Request; busy: boolean; run: (f: () => Promise<void>) => void };

function ToolCall({ r, busy, run }: Act) {
  const say = (a: string) => run(() => answer(r.id, [a]));
  return (
    <>
      <div className="request-title">The agent wants to {intent(r.summary ?? r.tool)}</div>
      {r.reason && <div className="muted small">{r.reason}</div>}
      {r.preview && r.preview.length > 0 && (
        <pre className="diff">
          {r.preview.map((l, i) => (
            <div key={i} className={l.mark === "+" ? "add" : l.mark === "-" ? "del" : "ctx"}>
              {l.mark === "+" || l.mark === "-" ? l.mark : " "} {l.line}
            </div>
          ))}
        </pre>
      )}
      <div className="request-actions">
        <button className="primary" disabled={busy} onClick={() => say("yes")}>
          Allow
        </button>
        <button className="secondary" disabled={busy} onClick={() => say("always")} title="Don't ask again about this kind of step in this task">
          Allow for this task
        </button>
        {r.rule && (
          <button className="secondary" disabled={busy} onClick={() => say("rule")} title={`Saved as a lasting rule for every task: ${r.rule}`}>
            Always allow
          </button>
        )}
        <button className="danger" disabled={busy} onClick={() => say("no")}>
          Don't allow
        </button>
      </div>
    </>
  );
}

/** One answer per question: picked option numbers (`1,3`), the person's own words, or `-`. */
function Questions({ r, busy, run }: Act) {
  const qs = r.questions ?? [];
  const [picked, setPicked] = useState<number[][]>(() => qs.map(() => []));
  const [words, setWords] = useState<string[]>(() => qs.map(() => ""));
  const toAnswer = (q: Question, i: number) =>
    q.secret ? "-" : words[i].trim() || (picked[i].length ? picked[i].map((n) => n + 1).join(",") : "");
  const ready = qs.every((q, i) => toAnswer(q, i) !== "");
  const pick = (i: number, n: number, multi: boolean) =>
    setPicked((p) =>
      p.map((sel, j) => (j !== i ? sel : multi ? (sel.includes(n) ? sel.filter((x) => x !== n) : [...sel, n]) : [n])),
    );
  return (
    <>
      <div className="request-title">The agent has a question</div>
      {qs.map((q, i) => (
        <fieldset key={q.id} className="question">
          <legend>
            {q.header && <span className="tag">{q.header}</span>} <Linked text={q.question} />
          </legend>
          {q.secret ? (
            <p className="muted small">
              anchorleg doesn't pass secrets (they'd be stored on disk). Give the agent the value another way, e.g. an
              environment variable; this question is answered with “-”.
            </p>
          ) : (
            <>
              {(q.options ?? []).map((o, n) => (
                <label key={n} className="option">
                  <input
                    type={q.multi_select ? "checkbox" : "radio"}
                    name={`q-${r.id}-${i}`}
                    checked={picked[i].includes(n)}
                    disabled={busy || words[i].trim() !== ""}
                    onChange={() => pick(i, n, !!q.multi_select)}
                  />
                  <span>
                    {o.label}
                    {o.description && <span className="muted"> — {o.description}</span>}
                  </span>
                </label>
              ))}
              <input
                className="own-words"
                placeholder={q.options?.length ? "…or in your own words" : "Your answer"}
                value={words[i]}
                disabled={busy}
                onChange={(e) => setWords((w) => w.map((x, j) => (j === i ? e.target.value : x)))}
              />
            </>
          )}
        </fieldset>
      ))}
      <div className="request-actions">
        <button className="primary" disabled={busy || !ready} onClick={() => run(() => answer(r.id, qs.map(toAnswer)))}>
          Send answer
        </button>
        <button className="secondary" disabled={busy} onClick={() => run(() => answer(r.id, [], true))} title="The agent is told you didn't answer and decides itself">
          Let the agent decide
        </button>
      </div>
    </>
  );
}

function Plan({ r, busy, run }: Act) {
  const original = r.plan ?? "";
  const [text, setText] = useState(original);
  const [changes, setChanges] = useState("");
  const [editing, setEditing] = useState(false);
  const edited = text.trim() !== original.trim();
  return (
    <>
      <div className="request-title">The plan is ready. Approve it?</div>
      <p className="muted small">
        Nothing has changed yet. You can{" "}
        <button className="link" disabled={busy} onClick={() => setEditing(!editing)}>
          {editing ? "see it as written" : "edit the plan"}
        </button>{" "}
        before approving it.
      </p>
      {editing ? (
        <textarea className="plan" value={text} rows={Math.min(16, Math.max(4, text.split("\n").length))} disabled={busy} onChange={(e) => setText(e.target.value)} />
      ) : (
        <div className="plan-view">
          <Markdown source={text} cwd={r.cwd} />
        </div>
      )}
      <div className="request-actions">
        <button className="primary" disabled={busy} onClick={() => run(() => (edited ? approvePlan(r.id, text) : answer(r.id, ["Approve"])))}>
          {edited ? "Approve my version" : "Approve and start"}
        </button>
        <input
          className="own-words"
          placeholder="…or say what to change"
          value={changes}
          disabled={busy}
          onChange={(e) => setChanges(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && changes.trim() && run(() => answer(r.id, [changes.trim()]))}
        />
        <button className="secondary" disabled={busy} onClick={() => run(() => answer(r.id, [changes.trim() || "Keep planning"]))}>
          {changes.trim() ? "Ask for these changes" : "Keep planning"}
        </button>
      </div>
    </>
  );
}

function Countdown({ deadline }: { deadline: number | null }) {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    if (deadline == null) return;
    const t = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => clearInterval(t);
  }, [deadline]);
  if (deadline == null) return <span className="muted small">waits for you</span>;
  const left = Math.max(0, Math.round(deadline - now));
  const m = Math.floor(left / 60);
  return (
    <span className={`small ${left < 120 ? "error" : "muted"}`}>
      {left === 0 ? "timing out" : `answer within ${m > 0 ? `${m}m ` : ""}${left % 60}s`}
    </span>
  );
}

/** `Write  src/a.rs` → "create src/a.rs". */
function intent(what: string): string {
  const [tool, ...rest] = what.split("  ");
  const detail = rest.join("  ").trim();
  const verb: Record<string, string> = {
    Bash: "run",
    Write: "create",
    Edit: "change",
    MultiEdit: "change",
    NotebookEdit: "change",
    Read: "read",
    WebFetch: "open",
    WebSearch: "search the web for",
    Permissions: "get more access:",
  };
  const v = verb[tool];
  if (v) return `${v} ${detail}`;
  if (tool.startsWith("mcp__")) return `use ${tool.split("__").slice(1).join(" ")}${detail ? ` (${detail})` : ""}`;
  return what;
}
