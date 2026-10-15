// Runs → sessions. A reply or a fork is a new run whose `parent_id` is the run it continues, and
// `sessions show <run>` shows the whole chain up to that run, so the list shows each chain's
// newest run: the runs nothing in the list continues.
import type { Run } from "./api";

export function heads(runs: Run[]): Run[] {
  // A side question (`btw`) continues nothing: it shows beside its session, not in the list.
  const main = runs.filter((r) => !r.side);
  const continued = new Set(main.map((r) => r.parent_id).filter((p) => p != null));
  return main.filter((r) => !continued.has(r.id));
}

/** The side questions asked of any run in `chain`, newest first. */
export const sideRuns = (runs: Run[], chain: number[]) => runs.filter((r) => r.side && r.parent_id != null && chain.includes(r.parent_id));

/** A side question's task without the note anchorleg adds for the agent. */
export const question = (task: string) => task.replace(/\n\n\(A side question about this session[\s\S]*\)\s*$/, "");

/** The newest run continuing `id`, or `id` when nothing does (a reply sent from the terminal). */
export function newest(runs: Run[], id: number): number {
  const byId = new Map(runs.map((x) => [x.id, x]));
  const continues = (r: Run) => {
    for (let p = r.parent_id; p != null; p = byId.get(p)?.parent_id ?? null) if (p === id) return true;
    return false;
  };
  return runs.reduce((best, r) => (r.id > best && !r.side && continues(r) ? r.id : best), id);
}

/** The session's name: the nearest one given on this run or a run it continues. */
export function nameOf(runs: Run[], r: Run): string | null {
  const byId = new Map(runs.map((x) => [x.id, x]));
  for (let x: Run | undefined = r; x; x = x.parent_id != null ? byId.get(x.parent_id) : undefined)
    if (x.name) return x.name;
  return null;
}

/** The account the session started on: its first run's. */
export function startedOn(runs: Run[], r: Run): string | null {
  const byId = new Map(runs.map((x) => [x.id, x]));
  let first = r;
  for (let x: Run | undefined = r; x; x = x.parent_id != null ? byId.get(x.parent_id) : undefined) first = x;
  return first.account ?? r.account;
}

/** Hues for accounts, in the order anchorleg tries them; the brand green is left out. */
const HUES = [250, 25, 300, 85, 200, 345, 130, 55];

/** A steady hue per account: by its place in the account list, so colours don't shuffle. */
export function accountHue(names: string[], account: string | null | undefined): number | null {
  if (!account) return null;
  const i = names.indexOf(account);
  if (i >= 0) return HUES[i % HUES.length];
  let h = 0;
  for (const c of account) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return HUES[h % HUES.length];
}

export const isWorking = (r: Run) => (r.state === "running" || r.state === "waiting" || r.state === "switching") && r.pid != null;

export function ago(secs: number): string {
  const d = Math.max(0, Math.floor(Date.now() / 1000 - secs));
  if (d < 60) return `${d}s`;
  if (d < 3600) return `${Math.floor(d / 60)}m`;
  if (d < 86400) return `${Math.floor(d / 3600)}h`;
  return `${Math.floor(d / 86400)}d`;
}

/** `~/repo` for paths under the home folder. */
export function tilde(path: string): string {
  const m = path.match(/^\/(Users|home)\/[^/]+/);
  return m ? "~" + path.slice(m[0].length) : path;
}

export function short(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n % 1_000_000 ? 1 : 0)}M`;
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}
