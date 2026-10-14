// Starting a task, as a form: a folder, what to do, files, and the options as plain choices.
// Everything here is an `anchorleg run` option; nobody has to know that.
import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { cliSettings, start, type Account, type Run, type Status } from "./api";
import { windowName } from "./Accounts";
import { nextAccount, Ring, roomLeft, VENDORS } from "./Limits";
import { pastedImages } from "./paste";
import { tilde } from "./sessions";
import { APPROVAL, Field, Icon, Menu, Switch } from "./ui";

export default function NewTask({
  folder,
  setFolder,
  runs,
  accounts,
  status,
  onStarted,
}: {
  folder: string;
  setFolder: (f: string) => void;
  /** For the recent folders. */
  runs: Run[];
  accounts: Account[];
  /** For the line under the greeting: where the task would start and the room there. */
  status: Status | null;
  onStarted: (run: number) => void;
}) {
  const [task, setTask] = useState("");
  const [attach, setAttach] = useState<string[]>([]);
  const [more, setMore] = useState(false);
  const [mode, setMode] = useState("");
  const [plan, setPlan] = useState(false);
  const [goal, setGoal] = useState("");
  const [account, setAccount] = useState("");
  const [onlyThat, setOnlyThat] = useState(false);
  const [worktree, setWorktree] = useState(false);
  const [name, setName] = useState("");
  const [model, setModel] = useState("");
  const [cli, setCli] = useState("");
  const [policy, setPolicy] = useState("");
  const [budget, setBudget] = useState("");
  const [goalTurns, setGoalTurns] = useState("");
  const [answerWait, setAnswerWait] = useState("");
  const [models, setModels] = useState<Record<string, string | null>>({});
  useEffect(() => {
    cliSettings()
      .then((s) => setModels(Object.fromEntries(Object.entries(s.vendors).map(([v, x]) => [v, x.model]))))
      .catch(() => {});
  }, []);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const off = getCurrentWebview().onDragDropEvent((e) => {
      const d = e.payload;
      if (d.type === "drop") setAttach((a) => [...a, ...d.paths.filter((p) => !a.includes(p))]);
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  const recent = [...new Set(runs.map((r) => r.cwd))].slice(0, 6);
  const choose = async () => {
    const picked = await open({ directory: true, defaultPath: folder || undefined });
    if (typeof picked === "string") setFolder(picked);
  };
  const addFiles = async () => {
    const picked = await open({ multiple: true, defaultPath: folder || undefined });
    if (picked) setAttach((a) => [...a, ...[picked].flat().filter((p) => !a.includes(p))]);
  };
  const ready = folder.trim() !== "" && (task.trim() !== "" || goal.trim() !== "");
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await start({
        text: task.trim(),
        cwd: folder.trim(),
        attach,
        mode: mode || undefined,
        plan,
        goal: goal.trim() || undefined,
        prefer: account && !onlyThat ? account : undefined,
        account: account && onlyThat ? account : undefined,
        worktree,
        name: name.trim() || undefined,
        model: model.trim() || undefined,
        cli: cli || undefined,
        policy: policy || undefined,
        budget: budget.trim() || undefined,
        goal_turns: goal.trim() && Number(goalTurns) > 0 ? Number(goalTurns) : undefined,
        answer_wait: answerWait || undefined,
      });
      setTask("");
      setAttach([]);
      onStarted(r.run_id);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const options = [mode, plan, goal.trim(), account, worktree, name.trim(), model.trim(), cli, policy, budget.trim(), answerWait].filter(Boolean).length;
  // Where it would start: the account picked here, else the one the next run would use.
  const starts = account ? status?.accounts.find((a) => a.name === account) ?? null : nextAccount(status);
  const startModel = model.trim() || (starts ? models[starts.vendor] : null);
  const folderName = folder.split("/").filter(Boolean).pop();
  return (
    <div className="page new-task">
      <h2 className="page-title">What's on your mind?</h2>
      <p className="start-line">
        {starts ? (
          <>
            <span className={`level-${(roomLeft(starts) ?? 1) <= 0.05 ? "full" : (roomLeft(starts) ?? 1) <= 0.2 ? "high" : "ok"}`}>
              <Ring left={roomLeft(starts)} />
            </span>
            <span>
              Starts on <b>{starts.name}</b>
            </span>
            <span className="dot-sep">{VENDORS[starts.vendor] ?? starts.vendor}</span>
            <span className="dot-sep">{startModel || "default model"}</span>
            {starts.windows.map((w) => (
              <span key={w.window} className="dot-sep">
                {windowName(w.window)} {w.used == null ? "?" : `${Math.round((1 - w.used) * 100)}% left`}
              </span>
            ))}
          </>
        ) : status?.next.action === "no_accounts" ? (
          <span>Add an account first: Accounts in the limits menu at the top.</span>
        ) : (
          <span className="muted">It works in the background and keeps going if you close this window.</span>
        )}
      </p>

      <div className="task-card">
        {attach.length > 0 && (
          <div className="chips">
            {attach.map((p) => (
              <span key={p} className="chip" title={p}>
                <Icon name="clip" size={12} /> {p.split("/").pop()}
                <button className="link" onClick={() => setAttach(attach.filter((x) => x !== p))} aria-label="Remove">
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
        <textarea
          className="task-box"
          autoFocus
          rows={4}
          value={task}
          placeholder="e.g. Fix the failing login tests and explain what was wrong (paste screenshots here too)"
          onChange={(e) => setTask(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && (e.metaKey || e.ctrlKey) && ready && !busy && go()}
          onPaste={(e) =>
            pastedImages(e)?.then(
              (paths) => setAttach((a) => [...a, ...paths]),
              (err) => setError(String(err)),
            )
          }
        />
        <div className="task-bar">
          <Menu
            label={folderName ?? "Choose a project folder"}
            icon="folder"
            align="left"
            items={[
              ...recent.map((f) => ({ label: f.split("/").filter(Boolean).pop() ?? f, hint: tilde(f), icon: f === folder ? "check" : "folder", onClick: () => setFolder(f) })),
              ...(recent.length ? (["-"] as const) : []),
              { label: "Choose another folder…", icon: "plus", onClick: choose },
            ]}
          />
          <button className="icon-text" onClick={addFiles} aria-label="Attach files" data-tip="Attach files (or drop them on the window)">
            <Icon name="clip" />
          </button>
          <button className={`icon-text ${more ? "on" : ""}`} onClick={() => setMore(!more)} aria-expanded={more}>
            <Icon name="sliders" /> Options{options > 0 && <span className="count">{options}</span>}
          </button>
          <span className="spacer" />
          <button className="primary" disabled={!ready || busy} onClick={go} data-tip={folder ? "⌘⏎" : "Choose a project folder first"}>
            <Icon name="send" /> {busy ? "Starting…" : plan ? "Make a plan" : "Start"}
          </button>
        </div>
      </div>

      {more && (
        <div className="options">
          <Field label="Approvals" hint={APPROVAL.find((a) => a.value === mode)?.hint}>
            <select value={mode} onChange={(e) => setMode(e.target.value)}>
              {APPROVAL.map((a) => (
                <option key={a.value} value={a.value}>
                  {a.label}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Account">
            <select value={account} onChange={(e) => setAccount(e.target.value)}>
              <option value="">Automatic (recommended)</option>
              {accounts
                .filter((a) => a.enabled !== false)
                .map((a) => (
                  <option key={a.name} value={a.name}>
                    Start on {a.name} ({a.vendor})
                  </option>
                ))}
            </select>
          </Field>
          <Field label="Keep going until" hint="Checked after each turn.">
            <input value={goal} placeholder="e.g. all tests pass" onChange={(e) => setGoal(e.target.value)} />
          </Field>
          <Field label="Name">
            <input value={name} placeholder="e.g. login fix" onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Agent">
            <select value={cli} onChange={(e) => setCli(e.target.value)}>
              <option value="">Any (recommended)</option>
              {Object.entries(VENDORS).map(([v, l]) => (
                <option key={v} value={v}>
                  Only {l}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Model">
            <input value={model} placeholder={(starts && models[starts.vendor]) || "The agent's default"} onChange={(e) => setModel(e.target.value)} />
          </Field>
          <Field label="Choosing accounts">
            <select value={policy} onChange={(e) => setPolicy(e.target.value)}>
              <option value="">My usual setting</option>
              <option value="drain">Use one up before the next</option>
              <option value="balance">Spread use evenly</option>
              <option value="save-weekly">Save weekly limits</option>
            </select>
          </Field>
          <Field label="Token budget" hint="It stops at a clean point once this is used.">
            <input value={budget} placeholder="No limit, or e.g. 500k" onChange={(e) => setBudget(e.target.value)} />
          </Field>
          <Field label="If I don't answer, go on after">
            <select value={answerWait} onChange={(e) => setAnswerWait(e.target.value)}>
              <option value="">Usual: questions wait, approvals 30 min</option>
              <option value="2m">2 minutes</option>
              <option value="10m">10 minutes</option>
              <option value="30m">30 minutes</option>
              <option value="2h">2 hours</option>
            </select>
          </Field>
          {goal.trim() && (
            <Field label="Turns toward the goal">
              <input type="number" min={1} value={goalTurns} placeholder="10" onChange={(e) => setGoalTurns(e.target.value)} />
            </Field>
          )}
          <div className="switches">
            <Switch checked={plan} onChange={setPlan} label="Show me a plan first" hint="Nothing changes until you approve it." />
            <Switch checked={worktree} onChange={setWorktree} label="Work on a separate copy" hint="Its own git branch, so it can't get in your way." />
            {account && <Switch checked={onlyThat} onChange={setOnlyThat} label={`Only use ${account}`} hint="Never move to another account." />}
          </div>
        </div>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  );
}
