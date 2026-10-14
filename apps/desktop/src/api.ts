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
  /** A side question's run (`btw`): it continues nothing. */
  side?: boolean;
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
  /** A side question: a read-only copy of that session answers. */
  btw?: number;
  /** Go back to before that run (conversation and files, unless `keep_files`). */
  rewind?: number;
  keep_files?: boolean;
  /** Review that session's changes on another account; `fix` sends the findings back. */
  review?: number;
  fix?: boolean;
  goal?: string;
  mode?: string;
  plan?: boolean;
  /** Start on this account; `account`: use only it. */
  prefer?: string;
  account?: string;
  /** Work in a git worktree of its own. */
  worktree?: boolean;
  name?: string;
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

// ---- Accounts, quota, usage, settings (M3) ------------------------------------------------------

export interface WindowStatus {
  /** `five_hour`, `seven_day`, … */
  window: string;
  status: string | null;
  /** 0–1; 0 once `resets_at` has passed. */
  used: number | null;
  resets_at: number | null;
  source: string;
  /** When it was read. */
  updated_at: number;
}

export interface AccountStatus {
  name: string;
  vendor: string;
  priority: number;
  blocked_until: number | null;
  windows: WindowStatus[];
  credits: { state: "off" | "available" | "in_use" | "unlimited"; balance: number | null; reason: string | null; updated_at: number } | null;
  credits_use: "never" | "last-resort" | "allow";
}

/** `anchorleg status --json`. */
export interface Status {
  now: number;
  policy: string;
  next: { action: "use"; account: string; reason: string } | { action: "wait"; until: number; account: string } | { action: "no_accounts" };
  accounts: AccountStatus[];
}

/** One `accounts list --json` entry; fields left at their default are missing. */
export interface Account {
  name: string;
  vendor: string;
  priority: number;
  enabled?: boolean;
  bin?: string;
  env?: Record<string, string>;
  args?: string[];
  token_from?: string;
  shell?: string;
  reserve_7d?: number;
  stop_at?: number;
  strict?: boolean;
  pinned?: boolean;
  credits?: string;
}

export interface Usage {
  now: number;
  since: number;
  totals: {
    account: string;
    model: string | null;
    launches: number;
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    cache_write_tokens: number;
    cost_usd: number | null;
    credits_cost_usd: number | null;
  }[];
  /** `[day start, tokens, list-price cost]`. */
  by_day: [number, number, number][];
}

export interface PolicyInfo {
  policy: string;
  policies: { name: string; describe: string }[];
}

export interface VendorSettings {
  model: string | null;
  effort: string | null;
  args: string[];
}

export interface AccountChange {
  name: string;
  priority?: number;
  reserve_7d?: number;
  stop_at?: number;
  strict?: boolean;
  pinned?: boolean;
  enabled?: boolean;
  credits?: string;
}

export const status = (refresh = false) => invoke<Status>("status", { refresh });
export const accounts = () => invoke<Account[]>("accounts");
export const usage = (since: string) => invoke<Usage>("usage", { since });
export const policy = (set?: string) => invoke<PolicyInfo>("policy", { set: set ?? null });
export const cliSettings = (change?: { vendor: string; model?: string; effort?: string; args?: string }) =>
  invoke<{ vendors: Record<string, VendorSettings> }>("settings", { change: change ?? null });
export const setAccounts = (changes: AccountChange[]) => invoke<void>("set_accounts", { changes });
export const addAccount = (name: string, command: string) => invoke<void>("add_account", { name, command });
export const removeAccount = (name: string) => invoke<void>("remove_account", { name });
export const importAliases = (yes: boolean) => invoke<{ add: Account[]; skip: string[]; saved: boolean }>("import_aliases", { yes });
export const updateTray = (title: string | null, lines: string[], waiting: number) =>
  invoke<void>("update_tray", { title, lines, waiting });

// ---- Session tools and the rest of the settings (M4) ----------------------------------------------

/** What a session changed since it started, as a unified diff (git folders). */
export const diff = (run: number) => invoke<string>("diff", { run });
/** `preview` → `{preview}` (diff --stat, or null: no changes); `undo` / `redo` → `{done}`. */
export const undo = (run: number, action: "preview" | "undo" | "redo") =>
  invoke<{ preview?: string | null; done?: string }>("undo", { run, action });
/** `!cmd` in the session's folder; the output goes with the next message. */
export const shell = (cwd: string, command: string) => invoke<string>("shell", { cwd, command });
export const rules = (add: string[] = [], rm: string[] = []) => invoke<{ rules: string[] }>("rules", { add, rm });
export interface NotifySettings {
  desktop: boolean;
  command: string | null;
  data_dir: string;
  app_running: boolean;
}
export const notifySettings = (change?: { desktop?: boolean; command?: string; test?: boolean }) =>
  invoke<NotifySettings>("notify", { change: change ?? null });
export interface McpSync {
  accounts: { name: string; servers: string[] }[];
  plan: { server: string; account: string; skip: string | null }[];
  results: { server: string; account: string; ok: boolean; error: string | null }[] | null;
}
export const mcpSync = (withEnv: boolean, yes: boolean) => invoke<McpSync>("mcp_sync", { withEnv, yes });
