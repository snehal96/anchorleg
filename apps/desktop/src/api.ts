// The Rust commands in src-tauri/src/lib.rs. Shapes follow the CLI's `--json` output.
import { invoke } from "@tauri-apps/api/core";

export interface CliInfo {
  path: string | null;
  version: string | null;
  /** The app's own copy of the CLI. */
  bundled: boolean;
  ok: boolean;
  min_version: string;
  install_command: string;
  problem: string | null;
}

export type RunState = "running" | "switching" | "waiting" | "done" | "failed" | "stopped";

/** One entry of `anchorleg sessions --json`. Times are Unix seconds. */
export interface Run {
  id: number;
  task: string;
  cwd: string;
  account: string | null;
  session_id: string | null;
  state: RunState;
  started_at: number;
  ended_at: number | null;
  /** The run this one follows up or forks from. */
  parent_id: number | null;
  pid: number | null;
  mode: string | null;
  goal: string | null;
  goal_status: string | null;
  review_of: number | null;
  /** Given on this run; follow-ups inherit it (see `Session.name`). */
  name: string | null;
}

export const cliInfo = () => invoke<CliInfo>("cli_info");
export const sessions = (limit = 50) => invoke<Run[]>("sessions", { limit });

export type Role = "user" | "agent" | "tool" | "tool_error" | "output" | "thinking" | "anchorleg" | "error";
export interface Message {
  role: Role;
  text: string;
}

/** `anchorleg sessions show <run> --json` (docs/CLI.md). */
export interface Session {
  schema: number;
  run: Run;
  /** Every run in the session, oldest first. */
  runs: number[];
  name: string | null;
  /** The run's process is alive. */
  working: boolean;
  /** `messages` replace the copy from this index on. */
  since: number;
  next: number;
  messages: Message[];
  running: { what: string; output: string } | null;
  subagents: { id: string; name: string; done: boolean; steps: number; doing: string | null }[];
  steps: { text: string; status: "pending" | "in_progress" | "done" }[];
  context: { tokens: number | null; window: number | null };
  slash_commands: string[] | null;
}

export const session = (run: number, since = 0) => invoke<Session>("session", { run, since });

export interface Start {
  text: string;
  cwd?: string;
  follow_up?: number;
  fork?: number;
  attach?: string[];
}
/** `anchorleg run --detach --json`: returns once the run has an id. */
export const start = (req: Start) => invoke<{ run_id: number; pid: number }>("start", { req });
/** A message for a working session; `interrupt` stops its turn first. */
export const send = (run: number, text: string, interrupt = false, attach: string[] = []) =>
  invoke<void>("send", { run, text, interrupt, attach });
export const stop = (run: number) => invoke<void>("stop", { run });
export const rename = (run: number, name: string) => invoke<void>("rename", { run, name });
export const files = (cwd: string) => invoke<string[]>("files", { cwd });

/** `~/.local/bin/anchorleg`, the optional terminal command (D31). */
export type TerminalState =
  | { state: "unavailable"; reason: string }
  | { state: "linked" }
  | { state: "missing" }
  | { state: "older"; version: string | null }
  | { state: "other"; version: string | null };

export type Terminal = TerminalState & {
  link: string;
  app_version: string | null;
  /** What a new terminal runs for `anchorleg`, when the login shell answered. */
  shell_resolves: string | null;
  shell_version: string | null;
  /** `~/.local/bin` isn't on the login shell's PATH. */
  needs_path: boolean;
};

export const terminal = () => invoke<Terminal>("terminal");
export const linkTerminal = () => invoke<Terminal>("link_terminal");

export interface Question {
  id: string;
  header?: string;
  question: string;
  options?: { label: string; description?: string }[];
  multi_select?: boolean;
  /** anchorleg never passes secrets: answered with `-`. */
  secret?: boolean;
}

/** One entry of `anchorleg permission list --json` (docs/CLI.md). */
export interface Request {
  id: number;
  run_id: number;
  tool: string;
  reason: string | null;
  asked_at: number;
  /** Unix seconds when it stops waiting; null: as long as the run lives. */
  deadline: number | null;
  cwd: string;
  kind: "tool" | "question" | "plan";
  summary?: string;
  preview?: { mark: string; line: string }[];
  /** What "Always" saves as a lasting rule. */
  rule?: string;
  questions?: Question[];
  plan?: string | null;
}

export const permissions = () => invoke<Request[]>("permissions");
/** A tool call: `yes`, `always` (this session), `rule` or `no`. Questions: one answer each. */
export const answer = (id: number, answers: string[], skip = false) => invoke<void>("answer", { id, answers, skip });
export const approvePlan = (id: number, plan: string) => invoke<void>("approve_plan", { id, plan });

/** A run's notice (`desktop-notices.jsonl`), shown by the app while it's open (D65). */
export interface Notice {
  event: "waiting" | "done" | "failed" | "switched" | "blocked" | "test";
  run_id: number | null;
  title: string;
  message: string;
}
