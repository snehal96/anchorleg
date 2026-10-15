// What you can do with a task, as buttons and a menu; each opens a small dialog where it needs
// more than a click. They are the same CLI commands the slash commands use.
import { useState } from "react";
import { rename, start, stop, type Run, type Session } from "./api";
import type { ToolTab } from "./Tools";
import { conversationMarkdown, copyText } from "./share";
import { APPROVAL, Field, Icon, Menu, Modal, Switch, type MenuItem } from "./ui";
import { useVendors } from "./vendors";

type Dialog = "rename" | "side" | "review" | "rewind" | "fork" | "compact" | "goal" | "mode" | null;

export default function SessionActions({
  view,
  runs,
  mode,
  setMode,
  onTools,
  onStarted,
  onNew,
}: {
  view: Session;
  /** Every listed run, for the tasks of the runs to go back to. */
  runs: Run[];
  /** The approval setting for the next message (`null`: the task's own). */
  mode: string | null;
  setMode: (m: string | null) => void;
  onTools: (t: ToolTab) => void;
  onStarted: (run: number) => void;
  onNew: (cwd: string) => void;
}) {
  const [dialog, setDialog] = useState<Dialog>(null);
  const [error, setError] = useState<string | null>(null);
  const r = view.run;
  const working = view.working;
  const busyNote = working ? "Wait until it finishes, or stop it" : undefined;
  // What this task's CLI supports; unknown means everything is offered.
  const caps = useVendors().ofAccount(r.account);
  const can = (k: "fork" | "side" | "rewind") => caps?.[k] ?? true;
  const close = () => setDialog(null);

  return (
    <div className="actions">
      {working && (
        <button className="danger small" onClick={() => stop(r.id).catch((e) => setError(String(e)))} data-tip="Stop the agent now; reply later to go on">
          <Icon name="stop" size={14} /> Stop
        </button>
      )}
      <button className="icon-button" onClick={() => onTools("changes")} aria-label="See changes" data-tip="See changes">
        <Icon name="diff" />
      </button>
      <button className="icon-button" onClick={() => onTools("agents")} aria-label="Sub-agents" data-tip="Sub-agents: what each was asked and found">
        <Icon name="people" />
      </button>
      {can("side") && (
        <button className="icon-button" onClick={() => setDialog("side")} aria-label="Ask a side question" data-tip="Ask a side question">
          <Icon name="chat" />
        </button>
      )}
      <Menu
        label="More actions"
        icon="more"
        iconOnly
        items={[
          { label: "Rename…", icon: "pencil", onClick: () => setDialog("rename") },
          { label: "Get a second opinion…", hint: "Another account reviews the changes", icon: "review", disabled: working, onClick: () => setDialog("review") },
          { label: "Undo the changes…", hint: "Put the files back as they were", icon: "undo", disabled: working, onClick: () => onTools("changes") },
          ...(can("rewind")
            ? [{ label: "Go back to an earlier point…", hint: busyNote ?? "Conversation and files", icon: "back", disabled: working || view.runs.length < 2, onClick: () => setDialog("rewind") } as MenuItem]
            : []),
          "-",
          ...(can("fork") ? [{ label: "Try another direction…", hint: "A new task that starts from this conversation", icon: "fork", onClick: () => setDialog("fork") } as MenuItem] : []),
          { label: "Keep going until…", hint: busyNote ?? "Work until a condition holds", icon: "goal", disabled: working, onClick: () => setDialog("goal") },
          { label: "Approval setting…", hint: APPROVAL.find((a) => a.value === (mode ?? r.mode ?? ""))?.label, icon: "shield", onClick: () => setDialog("mode") },
          { label: "Free up memory", hint: busyNote ?? "Summarise the conversation so far", icon: "compact", disabled: working, onClick: () => setDialog("compact") },
          "-",
          { label: "Copy conversation", hint: "As text you can paste or share", icon: "copy", onClick: () => copyText(conversationMarkdown(view, view.name ?? r.task.split("\n")[0])).catch((e) => setError(String(e))) },
          { label: "New task in this folder", icon: "plus", onClick: () => onNew(r.cwd) },
        ]}
      />
      {error && <span className="error small">{error}</span>}

      {dialog === "rename" && <RenameDialog view={view} onClose={close} />}
      {dialog === "side" && <SideDialog view={view} onClose={close} onAsked={() => onTools("side")} />}
      {dialog === "review" && <ReviewDialog view={view} onClose={close} onStarted={onStarted} />}
      {dialog === "rewind" && <RewindDialog view={view} runs={runs} onClose={close} onStarted={onStarted} />}
      {dialog === "fork" && <ForkDialog view={view} onClose={close} onStarted={onStarted} />}
      {dialog === "goal" && <GoalDialog view={view} onClose={close} onStarted={onStarted} />}
      {dialog === "compact" && <CompactDialog view={view} onClose={close} onStarted={onStarted} />}
      {dialog === "mode" && <ModeDialog current={mode ?? r.mode ?? ""} onClose={close} onPick={(m) => (setMode(m), close())} />}
    </div>
  );
}

/** Runs `f`, shows its error, closes on success. */
function useAct(onClose: () => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const act = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
      onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  return { busy, error, act };
}

function Buttons({ busy, error, ok, onOk, onClose, disabled }: { busy: boolean; error: string | null; ok: string; onOk: () => void; onClose: () => void; disabled?: boolean }) {
  return (
    <>
      {error && <p className="error">{error}</p>}
      <div className="modal-actions">
        <button className="secondary" onClick={onClose}>
          Cancel
        </button>
        <button className="primary" disabled={busy || disabled} onClick={onOk}>
          {busy ? "…" : ok}
        </button>
      </div>
    </>
  );
}

function RenameDialog({ view, onClose }: { view: Session; onClose: () => void }) {
  const [name, setName] = useState(view.name ?? "");
  const { busy, error, act } = useAct(onClose);
  const ok = () => act(() => rename(view.run.id, name.trim()));
  return (
    <Modal title="Rename this task" onClose={onClose}>
      <Field label="Name" hint="Leave it empty to show the task's first message instead.">
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && ok()} />
      </Field>
      <Buttons busy={busy} error={error} ok="Save" onOk={ok} onClose={onClose} />
    </Modal>
  );
}

function SideDialog({ view, onClose, onAsked }: { view: Session; onClose: () => void; onAsked: () => void }) {
  const [q, setQ] = useState("");
  const { busy, error, act } = useAct(onClose);
  const ok = () =>
    act(async () => {
      await start({ text: q.trim(), btw: view.run.id });
      onAsked();
    });
  return (
    <Modal title="Ask a side question" onClose={onClose}>
      <p className="muted small">A read-only copy of this task answers; the task itself goes on untouched and nothing in the project changes.</p>
      <Field label="Your question">
        <textarea autoFocus rows={3} value={q} placeholder="e.g. Which database does this project use?" onChange={(e) => setQ(e.target.value)} />
      </Field>
      <Buttons busy={busy} error={error} ok="Ask" onOk={ok} onClose={onClose} disabled={!q.trim()} />
    </Modal>
  );
}

function ReviewDialog({ view, onClose, onStarted }: { view: Session; onClose: () => void; onStarted: (run: number) => void }) {
  const [fix, setFix] = useState(false);
  const [notes, setNotes] = useState("");
  const { busy, error, act } = useAct(onClose);
  const ok = () => act(async () => onStarted((await start({ text: notes.trim(), review: view.run.id, fix })).run_id));
  return (
    <Modal title="Get a second opinion" onClose={onClose}>
      <p className="muted small">Another account (or another agent) reads what this task changed and reports problems. It doesn't change anything itself.</p>
      <Switch checked={fix} onChange={setFix} label="Then send the findings back to fix" hint="This task gets the review as its next message and fixes what it found." />
      <Field label="Anything to look at in particular? (optional)">
        <textarea rows={2} value={notes} placeholder="e.g. Check the error handling" onChange={(e) => setNotes(e.target.value)} />
      </Field>
      <Buttons busy={busy} error={error} ok="Start the review" onOk={ok} onClose={onClose} />
    </Modal>
  );
}

export function RewindDialog({
  view,
  runs,
  onClose,
  onStarted,
  initial,
}: {
  view: Session;
  runs: Run[];
  onClose: () => void;
  onStarted: (run: number) => void;
  /** Opened from a message: go back to before its run (or to its point `at` inside run `to`),
   * maybe with its text to edit. */
  initial?: { to?: number; at?: string; message?: string; title?: string };
}) {
  // Going back to before the first run would be a new task; every later run is a point to go back to.
  const points = view.runs.slice(1);
  const [to, setTo] = useState<number>(initial?.to ?? points[points.length - 1]);
  const [keepFiles, setKeepFiles] = useState(false);
  const [message, setMessage] = useState(initial?.message ?? "");
  const { busy, error, act } = useAct(onClose);
  const task = (id: number) => runs.find((r) => r.id === id)?.task.split("\n")[0] ?? `#${id}`;
  const at = initial?.at;
  const ok = () => act(async () => onStarted((await start({ text: message.trim(), rewind: to, at, keep_files: keepFiles })).run_id));
  return (
    <Modal title={initial?.title ?? "Go back to an earlier point"} onClose={onClose}>
      {at ? (
        <p className="muted small">The conversation goes back to just before this message and goes on from there with what you send. The files stay as they are; the old conversation is kept.</p>
      ) : (
        <>
          <p className="muted small">The conversation goes back to just before the message you pick, and so do the project's files (unless you keep them). Then the agent goes on from there with your new message.</p>
          <Field label="Go back to before">
            <select value={to} onChange={(e) => setTo(Number(e.target.value))}>
              {points.map((id) => (
                <option key={id} value={id}>
                  “{task(id).slice(0, 70)}”
                </option>
              ))}
            </select>
          </Field>
          <Switch checked={keepFiles} onChange={setKeepFiles} label="Keep the files as they are now" hint="Only the conversation goes back." />
        </>
      )}
      <Field label="Then tell the agent">
        <textarea autoFocus={initial?.to != null} rows={3} value={message} placeholder="e.g. Let's try a simpler approach" onChange={(e) => setMessage(e.target.value)} />
      </Field>
      <Buttons busy={busy} error={error} ok={initial?.message ? "Send" : "Go back"} onOk={ok} onClose={onClose} disabled={!message.trim()} />
    </Modal>
  );
}

export function ForkDialog({
  view,
  from,
  onClose,
  onStarted,
}: {
  view: Session;
  /** Fork from this point (a reply's `at` in run `run`) instead of where the session is now. */
  from?: { run: number; at: string };
  onClose: () => void;
  onStarted: (run: number) => void;
}) {
  const [message, setMessage] = useState("");
  const { busy, error, act } = useAct(onClose);
  const ok = () => act(async () => onStarted((await start({ text: message.trim(), fork: from?.run ?? view.run.id, at: from?.at })).run_id));
  return (
    <Modal title="Try another direction" onClose={onClose}>
      <p className="muted small">
        {from ? "A new task that knows everything up to this reply." : "A new task that knows everything this one does so far."} This task stays as it is, so you can compare.
      </p>
      <Field label="What should the new task do?">
        <textarea autoFocus rows={3} value={message} placeholder="e.g. Try it with a different library" onChange={(e) => setMessage(e.target.value)} />
      </Field>
      <Buttons busy={busy} error={error} ok="Start new task" onOk={ok} onClose={onClose} disabled={!message.trim()} />
    </Modal>
  );
}

function GoalDialog({ view, onClose, onStarted }: { view: Session; onClose: () => void; onStarted: (run: number) => void }) {
  const [goal, setGoal] = useState(view.run.goal ?? "");
  const { busy, error, act } = useAct(onClose);
  const ok = () => act(async () => onStarted((await start({ text: "", follow_up: view.run.id, goal: goal.trim() })).run_id));
  return (
    <Modal title="Keep going until…" onClose={onClose}>
      <p className="muted small">The agent keeps working, checking after each turn, until this holds (or it decides it can't).</p>
      <Field label="Condition">
        <input autoFocus value={goal} placeholder="e.g. all tests pass and the README is updated" onChange={(e) => setGoal(e.target.value)} />
      </Field>
      <Buttons busy={busy} error={error} ok="Keep going" onOk={ok} onClose={onClose} disabled={!goal.trim()} />
    </Modal>
  );
}

function CompactDialog({ view, onClose, onStarted }: { view: Session; onClose: () => void; onStarted: (run: number) => void }) {
  const { busy, error, act } = useAct(onClose);
  const ok = () => act(async () => onStarted((await start({ text: "/compact", follow_up: view.run.id })).run_id));
  return (
    <Modal title="Free up memory" onClose={onClose}>
      <p className="muted small">
        Long conversations fill the agent's memory (its context{view.context.window && view.context.tokens != null ? `: ${Math.round((view.context.tokens * 100) / view.context.window)}% used` : ""}).
        The agent summarises everything so far and goes on from the summary. Nothing in the project changes.
      </p>
      <Buttons busy={busy} error={error} ok="Summarise now" onOk={ok} onClose={onClose} />
    </Modal>
  );
}

function ModeDialog({ current, onClose, onPick }: { current: string; onClose: () => void; onPick: (m: string | null) => void }) {
  const [pick, setPick] = useState(current);
  return (
    <Modal title="Approval setting" onClose={onClose}>
      <p className="muted small">What the agent may do without asking you, from your next message on.</p>
      {APPROVAL.map((a) => (
        <label key={a.value} className="choice">
          <input type="radio" name="mode" checked={pick === a.value} onChange={() => setPick(a.value)} />
          <span>
            <b>{a.label}</b>
            <span className="field-hint">{a.hint}</span>
          </span>
        </label>
      ))}
      <div className="modal-actions">
        <button className="secondary" onClick={onClose}>
          Cancel
        </button>
        <button className="primary" onClick={() => onPick(pick || null)}>
          Use this
        </button>
      </div>
    </Modal>
  );
}
