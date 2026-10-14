// What a session waits for, in its conversation: a tool call to approve, questions from the
// agent, or a plan. Answers go through `anchorleg permission answer` (docs/CLI.md).
import { useEffect, useState } from "react";
import { answer, approvePlan, type Question, type Request } from "./api";

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
      <div className="request-title">
        Allow <span className="mono">{r.summary ?? r.tool}</span>?
      </div>
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
        <button disabled={busy} onClick={() => say("yes")}>
          Yes
        </button>
        <button className="secondary" disabled={busy} onClick={() => say("always")} title="This tool, for the rest of this session">
          Yes, this session
        </button>
        {r.rule && (
          <button className="secondary" disabled={busy} onClick={() => say("rule")} title="Saved as a lasting rule for every session">
            Always <span className="mono small">{r.rule}</span>
          </button>
        )}
        <button className="danger" disabled={busy} onClick={() => say("no")}>
          No
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
      <div className="request-title">The agent asks</div>
      {qs.map((q, i) => (
        <fieldset key={q.id} className="question">
          <legend>
            {q.header && <span className="tag">{q.header}</span>} {q.question}
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
        <button disabled={busy || !ready} onClick={() => run(() => answer(r.id, qs.map(toAnswer)))}>
          Answer
        </button>
        <button className="secondary" disabled={busy} onClick={() => run(() => answer(r.id, [], true))} title="The agent is told you didn't answer">
          Skip
        </button>
      </div>
    </>
  );
}

function Plan({ r, busy, run }: Act) {
  const original = r.plan ?? "";
  const [text, setText] = useState(original);
  const [changes, setChanges] = useState("");
  const edited = text.trim() !== original.trim();
  return (
    <>
      <div className="request-title">Approve the plan?</div>
      <textarea className="plan" value={text} rows={Math.min(16, Math.max(4, text.split("\n").length))} disabled={busy} onChange={(e) => setText(e.target.value)} />
      <div className="request-actions">
        <button disabled={busy} onClick={() => run(() => (edited ? approvePlan(r.id, text) : answer(r.id, ["Approve"])))}>
          {edited ? "Approve as edited" : "Approve"}
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
          {changes.trim() ? "Send changes" : "Keep planning"}
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
