// Starting a task, as a form: a folder, what to do, files, and the options as plain choices.
// Everything here is an `anchorleg run` option; nobody has to know that.
import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { start, type Account, type Run } from "./api";
import { tilde } from "./sessions";
import { APPROVAL, Field, Icon, Menu, Switch } from "./ui";

export default function NewTask({
  folder,
  setFolder,
  runs,
  accounts,
  onStarted,
}: {
  folder: string;
  setFolder: (f: string) => void;
  /** For the recent folders. */
  runs: Run[];
  accounts: Account[];
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

  const options = [mode, plan, goal.trim(), account, worktree, name.trim()].filter(Boolean).length;
  const folderName = folder.split("/").filter(Boolean).pop();
  return (
    <div className="page new-task">
      <h2 className="page-title">What should the agent do?</h2>
      <p className="muted">It works in the background, on whichever account has room, and keeps going if you close this window.</p>

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
          placeholder="e.g. Fix the failing login tests and explain what was wrong"
          onChange={(e) => setTask(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && (e.metaKey || e.ctrlKey) && ready && !busy && go()}
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
      {folder && (
        <p className="folder-line muted small" title={folder}>
          <Icon name="folder" size={12} /> {tilde(folder)}
        </p>
      )}

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
