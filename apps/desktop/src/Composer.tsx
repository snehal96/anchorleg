import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { files, rename, send, shell, start, type Session } from "./api";
import { pastedImages } from "./paste";
import { short } from "./sessions";
import { Icon, Menu, Modal } from "./ui";
import { useVendors } from "./vendors";

/** Typed shortcuts for people who like them; every one is also a button or menu item. */
const OWN: [string, string][] = [
  ["clear", "a new task in this folder"],
  ["compact", "free up memory: summarise the conversation"],
  ["fork", "<message>: try another direction in a new task"],
  ["rename", "<name>: name this task"],
  ["btw", "<question>: a side question"],
  ["review", "[fix] [notes]: a second opinion from another account"],
  ["rewind", "<run> <message>: go back to before that run"],
  ["goal", "<condition>: keep going until it holds"],
];
const MAX_SUGGESTIONS = 8;

/** Codex has `/context` and `/init` only in its own app; anchorleg stands in for them. */
const CODEX_OWN: [string, string][] = [
  ["context", "how full the agent's memory is"],
  ["init", "write or improve AGENTS.md for this project"],
];

/** What Codex's `/init` asks for, sent as the message (as `anchorleg ui` does). */
const INIT_PROMPT =
  "Look through this repository and write an AGENTS.md at its root: a short guide for coding agents with the project's layout, how to build, test and lint it, and the conventions the code follows. Keep it under 300 lines; if an AGENTS.md exists, improve it instead of starting over.";

/**
 * The reply box of a task. Send replies (`run --follow-up`); while the agent works, a message
 * waits for the end of its step (`send`) or stops it and goes in now (`send --interrupt`).
 */
export default function Composer({
  view,
  mode,
  onSent,
  onStarted,
  onNew,
  onSide,
  draft,
}: {
  view: Session;
  /** Text put in the box from elsewhere (asking about a subagent); `n` changes each time. */
  draft?: { text: string; n: number } | null;
  /** The approval setting for this message (`null`: the task's own). */
  mode: string | null;
  /** The message went: the one-off settings are used up. */
  onSent: () => void;
  onStarted: (run: number) => void;
  onNew: (cwd: string) => void;
  onSide: () => void;
}) {
  const [text, setText] = useState("");
  const [attach, setAttach] = useState<string[]>([]);
  // Output of "Run a command", put in front of the next message, as `anchorleg ui` does with `!`.
  const [shellOut, setShellOut] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ text: string; error?: boolean } | null>(null);
  const [pick, setPick] = useState(0);
  const [dialog, setDialog] = useState<"mention" | "command" | null>(null);
  const box = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (!draft) return;
    setText(draft.text);
    requestAnimationFrame(() => {
      const el = box.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    });
  }, [draft?.n]);
  const folder = view.run.cwd;
  const working = view.working;
  const codex = useVendors().ofAccount(view.run.account)?.vendor === "codex";
  // Claude's guess at the next message: shown in an empty box, Tab puts it in.
  const guess = !working && view.suggestion ? view.suggestion : null;

  // Files dropped on the window go with the message.
  useEffect(() => {
    const off = getCurrentWebview().onDragDropEvent((e) => {
      const d = e.payload;
      if (d.type === "drop") setAttach((a) => [...a, ...d.paths.filter((p) => !a.includes(p))]);
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  // `@` completions and "Mention a file" read the folder's files once per folder.
  const [list, setList] = useState<{ folder: string; files: string[] } | null>(null);
  const atWord = text.match(/(^|\s)@(\S*)$/);
  const loadFiles = () => {
    if (list?.folder !== folder) files(folder).then((f) => setList({ folder, files: f }), () => setList({ folder, files: [] }));
  };
  useEffect(() => {
    if (atWord) loadFiles();
  }, [atWord != null, folder]);

  const suggestions = useMemo((): [string, string][] => {
    const slash = text.match(/^\/(\S*)$/);
    if (slash) {
      const typed = slash[1].toLowerCase();
      const own = codex ? [...OWN, ...CODEX_OWN] : OWN;
      const theirs = (view.slash_commands ?? []).filter((c) => !own.some(([n]) => n === c)).map((c): [string, string] => [c, "the agent's command"]);
      return [...own, ...theirs].filter(([n]) => n.toLowerCase().startsWith(typed)).slice(0, MAX_SUGGESTIONS);
    }
    if (atWord && list?.folder === folder) return match(list.files, atWord[2]).map((f): [string, string] => [f, ""]);
    return [];
  }, [text, view, atWord, list, folder, codex]);
  useEffect(() => setPick(0), [suggestions.length]);

  const complete = (choice: string) => {
    if (text.startsWith("/")) setText(`/${choice} `);
    else setText(text.replace(/@\S*$/, `@${choice} `));
    box.current?.focus();
  };
  const insert = (s: string) => {
    setText((t) => (t && !t.endsWith(" ") && !t.endsWith("\n") ? `${t} ${s}` : `${t}${s}`));
    box.current?.focus();
  };

  const attachFiles = async () => {
    const picked = await open({ multiple: true, defaultPath: folder || undefined });
    if (picked) setAttach((a) => [...a, ...[picked].flat().filter((p) => !a.includes(p))]);
  };

  const submit = async (interrupt = false) => {
    const t = text.trim();
    if (busy || (!t && attach.length === 0 && shellOut.length === 0)) return;
    setBusy(true);
    setNote(null);
    const done = (msg: string) => {
      setText("");
      setAttach([]);
      setNote({ text: msg });
    };
    // What a message sends: earlier command output, then the message.
    const withShell = (msg: string) => (shellOut.length ? `${shellOut.join("\n\n")}\n\n${msg}` : msg);
    const sent = () => {
      setShellOut([]);
      onSent();
    };
    try {
      const [cmd, ...rest] = t.startsWith("/") ? t.slice(1).split(" ") : [""];
      const arg = rest.join(" ").trim();
      if (t.startsWith("!")) {
        const command = t.slice(1).trim();
        const out = await shell(folder, command);
        setShellOut((o) => [...o, `$ ${command}\n${out}`]);
        done("The output goes with your next message.");
      } else if (codex && cmd === "context" && !arg) {
        const { tokens, window: w } = view.context;
        done(tokens == null ? "No token count yet: send a message first." : `Memory: ${short(tokens)}${w ? ` of ${short(w)} tokens (${Math.round((tokens * 100) / w)}%)` : " tokens"}.`);
      } else if (codex && cmd === "init" && !arg) {
        if (working) throw "Wait until it finishes, or stop it first.";
        const r = await start({ text: withShell(INIT_PROMPT), follow_up: view.run.id, mode: mode ?? undefined });
        sent();
        done("Asked it to write AGENTS.md.");
        onStarted(r.run_id);
      } else if (cmd === "clear") {
        done("");
        onNew(folder);
      } else if (cmd === "rename") {
        await rename(view.run.id, arg);
        done(arg ? `Named “${arg}”.` : "Name cleared.");
      } else if (cmd === "fork") {
        if (!arg) throw "/fork <message>";
        const r = await start({ text: arg, fork: view.run.id, attach });
        done(`Started a new task (#${r.run_id}).`);
        onStarted(r.run_id);
      } else if (cmd === "btw") {
        if (!arg) throw "/btw <question>";
        await start({ text: arg, btw: view.run.id });
        done("Asked. The answer shows under Side questions.");
        onSide();
      } else if (cmd === "review") {
        if (working) throw "Wait until it finishes, or stop it first.";
        const fix = rest[0] === "fix";
        const r = await start({ text: (fix ? rest.slice(1) : rest).join(" ").trim(), review: view.run.id, fix });
        done(`Review started (#${r.run_id}).`);
        onStarted(r.run_id);
      } else if (cmd === "rewind") {
        const id = Number((rest[0] ?? "").replace(/^#/, ""));
        const message = rest.slice(1).join(" ").trim();
        if (!id || !message) throw "/rewind <run> <message>";
        if (working) throw "Wait until it finishes, or stop it first.";
        const r = await start({ text: withShell(message), rewind: id, attach });
        sent();
        done(`Went back; going on as #${r.run_id}.`);
        onStarted(r.run_id);
      } else if (cmd === "goal") {
        if (!arg) throw "/goal <condition>";
        if (working) throw "Wait until it finishes, or stop it first.";
        const r = await start({ text: "", follow_up: view.run.id, goal: arg, mode: mode ?? undefined });
        sent();
        done("Working toward the goal.");
        onStarted(r.run_id);
      } else if (working) {
        if (cmd === "compact") throw "Wait until it finishes, or stop it first.";
        await send(view.run.id, withShell(t), interrupt, attach);
        sent();
        done(interrupt ? "Stopping the current step; your message goes in now." : "Sent. It goes in when the current step ends.");
      } else {
        const r = await start({ text: withShell(t), follow_up: view.run.id, attach, mode: mode ?? undefined });
        sent();
        done("");
        onStarted(r.run_id);
      }
    } catch (e) {
      setNote({ text: String(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Tab" && guess && !text) {
      e.preventDefault();
      setText(guess);
      return;
    }
    if (suggestions.length > 0) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = suggestions.length;
        setPick((p) => (p + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !exact(text, suggestions[pick][0]))) {
        e.preventDefault();
        complete(suggestions[pick][0]);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit(e.metaKey || e.ctrlKey);
    }
  };

  const agentCommands = view.slash_commands ?? [];
  const empty = !text.trim() && attach.length === 0 && shellOut.length === 0;
  return (
    <div className="composer">
      {suggestions.length > 0 && (
        <ul className="suggestions">
          {suggestions.map(([n, d], i) => (
            <li key={n} className={i === pick ? "picked" : ""} onMouseDown={(e) => (e.preventDefault(), complete(n))}>
              <span className="mono">{text.startsWith("/") ? `/${n}` : n}</span>
              {d && <span className="muted"> {d}</span>}
            </li>
          ))}
        </ul>
      )}
      <div className="composer-box">
      {(attach.length > 0 || shellOut.length > 0) && (
        <div className="chips">
          {attach.map((p) => (
            <span key={p} className="chip" title={p}>
              <Icon name="clip" size={12} /> {p.split("/").pop()}
              <button className="link" onClick={() => setAttach(attach.filter((x) => x !== p))} aria-label="Remove">
                ×
              </button>
            </span>
          ))}
          {shellOut.map((o, i) => (
            <span key={i} className="chip mono" title={o}>
              <Icon name="terminal" size={12} /> {o.split("\n")[0].slice(2)}
              <button className="link" onClick={() => setShellOut(shellOut.filter((_, j) => j !== i))} aria-label="Remove">
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      <textarea
        ref={box}
        rows={Math.min(8, Math.max(2, text.split("\n").length))}
        value={text}
        placeholder={working ? "Write a message for the agent…" : guess ? `${guess}  (Tab)` : "Reply to the agent…"}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKey}
        onPaste={(e) =>
          pastedImages(e)?.then(
            (paths) => setAttach((a) => [...a, ...paths]),
            (err) => setNote({ text: String(err), error: true }),
          )
        }
        disabled={busy}
      />
      <div className="composer-bar">
        <button className="icon-text" onClick={attachFiles} title="Send files with your message (or drop them on the window)">
          <Icon name="clip" /> Attach
        </button>
        <button className="icon-text" onClick={() => (loadFiles(), setDialog("mention"))} title="Point the agent at a file in the project">
          <Icon name="file" /> Mention a file
        </button>
        <button className="icon-text" onClick={() => setDialog("command")} title="Run a command in the project folder and send its output with your message">
          <Icon name="terminal" /> Run a command
        </button>
        {agentCommands.length > 0 && (
          <Menu label="Agent commands" align="left" items={agentCommands.slice(0, 30).map((c) => ({ label: `/${c}`, onClick: () => setText(`/${c} `) }))} />
        )}
        <span className={`composer-note ${note?.error ? "error" : "muted"}`}>{note?.text}</span>
        <span className="spacer" />
        {working ? (
          <>
            <button className="secondary" disabled={busy || empty} onClick={() => submit(false)} title="Your message goes in when the current step ends">
              Send after this step
            </button>
            <button className="primary" disabled={busy || empty} onClick={() => submit(true)} title="Stop what the agent is doing and send your message now (⌘⏎)">
              Stop and send now
            </button>
          </>
        ) : (
          <button className="primary" disabled={busy || empty} onClick={() => submit()}>
            <Icon name="send" /> Send
          </button>
        )}
      </div>
      </div>
      {dialog === "mention" && (
        <MentionDialog
          files={list?.folder === folder ? list.files : null}
          onClose={() => setDialog(null)}
          onPick={(f) => {
            insert(`@${f} `);
            setDialog(null);
          }}
        />
      )}
      {dialog === "command" && (
        <CommandDialog
          folder={folder}
          onClose={() => setDialog(null)}
          onInclude={(o) => {
            setShellOut((s) => [...s, o]);
            setDialog(null);
          }}
        />
      )}
    </div>
  );
}

function match(all: string[], typed: string): string[] {
  const t = typed.toLowerCase();
  return all
    .filter((f) => f.toLowerCase().includes(t))
    .sort((a, b) => Number(!a.toLowerCase().startsWith(t)) - Number(!b.toLowerCase().startsWith(t)))
    .slice(0, MAX_SUGGESTIONS);
}

function MentionDialog({ files, onClose, onPick }: { files: string[] | null; onClose: () => void; onPick: (f: string) => void }) {
  const [q, setQ] = useState("");
  const found = files ? match(files, q) : [];
  return (
    <Modal title="Mention a file" onClose={onClose}>
      <p className="muted small">The agent will look at this file. Start typing its name.</p>
      <input autoFocus placeholder="e.g. login" value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && found[0] && onPick(found[0])} />
      <ul className="pick-list">
        {!files && <li className="muted">Reading the project…</li>}
        {files && found.length === 0 && <li className="muted">No file matches.</li>}
        {found.map((f) => (
          <li key={f}>
            <button onClick={() => onPick(f)}>
              <Icon name="file" size={13} /> {f}
            </button>
          </li>
        ))}
      </ul>
    </Modal>
  );
}

function CommandDialog({ folder, onClose, onInclude }: { folder: string; onClose: () => void; onInclude: (o: string) => void }) {
  const [command, setCommand] = useState("");
  const [out, setOut] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      setOut(await shell(folder, command.trim()));
    } catch (e) {
      setOut(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal title="Run a command" onClose={onClose} wide>
      <p className="muted small">
        Runs in the project folder (at most a minute) and shows you the output; you choose whether it goes to the agent with your next
        message. The agent doesn't run it.
      </p>
      <div className="add-row">
        <input className="mono grow" autoFocus placeholder="e.g. npm test" value={command} onChange={(e) => setCommand(e.target.value)} onKeyDown={(e) => e.key === "Enter" && command.trim() && run()} />
        <button className="secondary" disabled={busy || !command.trim()} onClick={run}>
          {busy ? "Running…" : "Run"}
        </button>
      </div>
      {out != null && <pre className="command-out">{out || "(no output)"}</pre>}
      <div className="modal-actions">
        <button className="secondary" onClick={onClose}>
          Close
        </button>
        <button className="primary" disabled={out == null} onClick={() => out != null && onInclude(`$ ${command.trim()}\n${out}`)}>
          Include with my next message
        </button>
      </div>
    </Modal>
  );
}

/** The text already is this completion (Enter then sends rather than completing again). */
function exact(text: string, choice: string): boolean {
  return text.trim() === `/${choice}` || text.endsWith(`@${choice}`);
}
