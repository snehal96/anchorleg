import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { files, rename, send, shell, start, type Session } from "./api";

/** anchorleg's own slash commands; the rest go to the session's CLI as the message. */
const OWN: [string, string][] = [
  ["clear", "a new session in this folder"],
  ["compact", "summarise the session to free context"],
  ["fork", "<message>: a new session from this one's history"],
  ["rename", "<name>: name this session"],
  ["btw", "<question>: a side question; a read-only copy answers"],
  ["review", "[fix] [notes]: review this session's changes on another account"],
  ["rewind", "<run> <message>: go back to before that run, files too"],
  ["goal", "<condition>: keep working until it holds"],
  ["mode", "ask | edits | auto: how tool calls are approved from the next message"],
];
/** For a new session. */
const NEW: [string, string][] = [
  ["plan", "<task>: plan first; you approve the plan before it runs"],
  ["goal", "<condition>: work until it holds"],
  ["mode", "ask | edits | auto: how tool calls are approved"],
];
const MODES = ["ask", "edits", "auto"];

const MAX_SUGGESTIONS = 8;

/**
 * The message box. Without a session (`view` null) it starts one in `cwd`; with one, Enter
 * sends into it: `anchorleg send` while it works (queued for the end of the turn, or
 * interrupting it), a reply (`run --follow-up`) once it's done.
 */
export default function Composer({
  view,
  cwd,
  seed,
  onStarted,
  onNew,
  onSide,
}: {
  view: Session | null;
  /** New sessions: the folder they run in. */
  cwd?: string;
  /** Text put in the box from elsewhere; a new `key` puts it again. */
  seed?: { text: string; key: number } | null;
  onStarted: (run: number) => void;
  onNew?: (cwd: string) => void;
  /** A side question was asked: show the answers. */
  onSide?: () => void;
}) {
  const [text, setText] = useState("");
  // `!cmd` output waiting to go in front of the next message, as in `anchorleg ui`.
  const [shellOut, setShellOut] = useState<string[]>([]);
  // `/mode`: for the next run started from here.
  const [mode, setMode] = useState<string | null>(null);
  useEffect(() => {
    if (seed) {
      setText(seed.text);
      box.current?.focus();
    }
  }, [seed?.key]);
  const [attach, setAttach] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ text: string; error?: boolean } | null>(null);
  const [pick, setPick] = useState(0);
  const box = useRef<HTMLTextAreaElement>(null);
  const folder = view?.run.cwd ?? cwd ?? "";
  const working = view?.working ?? false;

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

  // `@` completions read the folder's files once per folder.
  const [list, setList] = useState<{ folder: string; files: string[] } | null>(null);
  const atWord = text.match(/(^|\s)@(\S*)$/);
  useEffect(() => {
    if (atWord && folder && list?.folder !== folder)
      files(folder).then((f) => setList({ folder, files: f }), () => setList({ folder, files: [] }));
  }, [atWord != null, folder]);

  const suggestions = useMemo((): [string, string][] => {
    const slash = text.match(/^\/(\S*)$/);
    if (slash) {
      const typed = slash[1].toLowerCase();
      const own = (view ? OWN : NEW).map(([n, d]): [string, string] => [n, `anchorleg: ${d}`]);
      const theirs = (view?.slash_commands ?? [])
        .filter((c) => !OWN.some(([n]) => n === c))
        .map((c): [string, string] => [c, ""]);
      return [...own, ...theirs].filter(([n]) => n.toLowerCase().startsWith(typed)).slice(0, MAX_SUGGESTIONS);
    }
    if (atWord && list?.folder === folder) {
      const typed = atWord[2].toLowerCase();
      return list.files
        .filter((f) => f.toLowerCase().includes(typed))
        .sort((a, b) => Number(!a.toLowerCase().startsWith(typed)) - Number(!b.toLowerCase().startsWith(typed)))
        .slice(0, MAX_SUGGESTIONS)
        .map((f): [string, string] => [f, ""]);
    }
    return [];
  }, [text, view, atWord, list, folder]);
  useEffect(() => setPick(0), [suggestions.length]);

  const complete = (choice: string) => {
    if (text.startsWith("/")) setText(`/${choice} `);
    else setText(text.replace(/@\S*$/, `@${choice} `));
    box.current?.focus();
  };

  const attachFiles = async () => {
    const picked = await open({ multiple: true, defaultPath: folder || undefined });
    if (picked) setAttach((a) => [...a, ...[picked].flat().filter((p) => !a.includes(p))]);
  };

  const submit = async (interrupt = false) => {
    const t = text.trim();
    if (busy || (!t && attach.length === 0)) return;
    setBusy(true);
    setNote(null);
    const done = (msg: string) => {
      setText("");
      setAttach([]);
      setNote({ text: msg });
    };
    // What a run started from here sends: earlier `!cmd` output, then the message.
    const withShell = (msg: string) => (shellOut.length ? `${shellOut.join("\n\n")}\n\n${msg}` : msg);
    const sent = () => {
      setShellOut([]);
      setMode(null);
    };
    try {
      const [cmd, ...rest] = t.startsWith("/") ? t.slice(1).split(" ") : [""];
      const arg = rest.join(" ").trim();
      if (t.startsWith("!")) {
        if (!folder) throw "choose a folder first";
        const command = t.slice(1).trim();
        const out = await shell(folder, command);
        setShellOut((o) => [...o, `$ ${command}\n${out}`]);
        done("the output goes with your next message");
      } else if (cmd === "mode") {
        if (!MODES.includes(arg)) throw `/mode ${MODES.join(" | ")}`;
        setMode(arg);
        done(`mode ${arg} from the next message`);
      } else if (!view) {
        if (!folder) throw "choose a folder first";
        const plan = cmd === "plan";
        const goal = cmd === "goal" ? arg : undefined;
        const task = plan ? arg : goal != null ? "" : t;
        if (plan && !arg) throw "/plan <task>";
        if (cmd === "goal" && !arg) throw "/goal <condition>";
        const r = await start({ text: withShell(task), cwd: folder, attach, plan, goal, mode: mode ?? undefined });
        sent();
        done(`started #${r.run_id}`);
        onStarted(r.run_id);
      } else if (cmd === "btw") {
        if (!arg) throw "/btw <question>";
        await start({ text: arg, btw: view.run.id });
        done("asked a read-only copy; the session goes on untouched");
        onSide?.();
      } else if (cmd === "review") {
        if (working) throw "the session is working; review it once it's done";
        const fix = rest[0] === "fix";
        const notes = (fix ? rest.slice(1) : rest).join(" ").trim();
        const r = await start({ text: notes, review: view.run.id, fix });
        done(`reviewing as #${r.run_id}`);
        onStarted(r.run_id);
      } else if (cmd === "rewind") {
        const id = Number((rest[0] ?? "").replace(/^#/, ""));
        const message = rest.slice(1).join(" ").trim();
        if (!id || !message) throw "/rewind <run> <message>: the run to go back to before, and how to go on";
        if (working) throw "the session is working; stop it first";
        const r = await start({ text: withShell(message), rewind: id, attach });
        sent();
        done(`went back to before #${id}; going on as #${r.run_id}`);
        onStarted(r.run_id);
      } else if (cmd === "goal") {
        if (!arg) throw "/goal <condition>";
        if (working) throw "the session is working; set the goal once it's done";
        const r = await start({ text: "", follow_up: view.run.id, goal: arg, mode: mode ?? undefined });
        sent();
        done(`working toward the goal as #${r.run_id}`);
        onStarted(r.run_id);
      } else if (cmd === "clear") {
        done("");
        onNew?.(folder);
      } else if (cmd === "rename") {
        await rename(view.run.id, arg);
        done(arg ? `named “${arg}”` : "name cleared");
      } else if (cmd === "fork") {
        if (!arg) throw "/fork <message>: what the new session should do";
        const r = await start({ text: arg, fork: view.run.id, attach });
        done(`forked into #${r.run_id}`);
        onStarted(r.run_id);
      } else if (working) {
        if (cmd === "compact") throw "the session is working; /compact once it's done";
        await send(view.run.id, withShell(t), interrupt, attach);
        sent();
        done(interrupt ? "interrupting; sent" : "sent; it goes in when the turn ends");
      } else {
        const r = await start({ text: withShell(t), follow_up: view.run.id, attach, mode: mode ?? undefined });
        sent();
        done(`replied as #${r.run_id}`);
        onStarted(r.run_id);
      }
    } catch (e) {
      setNote({ text: String(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
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

  const placeholder = !view
    ? "What should the agent do?  ·  /plan  /goal  /mode  ·  !command"
    : working
      ? "Message (goes in when the turn ends; ⌘⏎ interrupts)"
      : "Reply  ·  / commands  ·  @ files  ·  !command";

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
      {(shellOut.length > 0 || mode) && (
        <div className="chips">
          {shellOut.map((o, i) => (
            <span key={i} className="chip mono" title={o}>
              {o.split("\n")[0]}
              <button className="link" onClick={() => setShellOut(shellOut.filter((_, j) => j !== i))} aria-label="Remove">
                ×
              </button>
            </span>
          ))}
          {mode && (
            <span className="chip">
              mode {mode}
              <button className="link" onClick={() => setMode(null)} aria-label="Remove">
                ×
              </button>
            </span>
          )}
        </div>
      )}
      {attach.length > 0 && (
        <div className="chips">
          {attach.map((p) => (
            <span key={p} className="chip mono" title={p}>
              {p.split("/").pop()}
              <button className="link" onClick={() => setAttach(attach.filter((x) => x !== p))} aria-label="Remove">
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
        placeholder={placeholder}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKey}
        disabled={busy}
      />
      <div className="composer-bar">
        <button className="link" onClick={attachFiles} title="Attach files (or drop them on the window)">
          Attach…
        </button>
        <span className={note?.error ? "error" : "muted"}>{note?.text}</span>
        <span className="spacer" />
        {working && (
          <button className="secondary" disabled={busy || !text.trim()} onClick={() => submit(true)}>
            Interrupt &amp; send
          </button>
        )}
        <button disabled={busy || (!text.trim() && attach.length === 0)} onClick={() => submit()}>
          {!view ? "Start" : working ? "Queue" : "Send"}
        </button>
      </div>
    </div>
  );
}

/** The text already is this completion (Enter then sends rather than completing again). */
function exact(text: string, choice: string): boolean {
  return text.trim() === `/${choice}` || text.endsWith(`@${choice}`);
}
