// Runs → sessions. A reply or a fork is a new run whose `parent_id` is the run it continues, and
// `sessions show <run>` shows the whole chain up to that run, so the list shows each chain's
// newest run: the runs nothing in the list continues.
import type { Run } from "./api";

export function heads(runs: Run[]): Run[] {
  const continued = new Set(runs.map((r) => r.parent_id).filter((p) => p != null));
  return runs.filter((r) => !continued.has(r.id));
}

/** The newest run continuing `id`, or `id` when nothing does (a reply sent from the terminal). */
export function newest(runs: Run[], id: number): number {
  const byId = new Map(runs.map((x) => [x.id, x]));
  const continues = (r: Run) => {
    for (let p = r.parent_id; p != null; p = byId.get(p)?.parent_id ?? null) if (p === id) return true;
    return false;
  };
  return runs.reduce((best, r) => (r.id > best && continues(r) ? r.id : best), id);
}

/** The session's name: the nearest one given on this run or a run it continues. */
export function nameOf(runs: Run[], r: Run): string | null {
  const byId = new Map(runs.map((x) => [x.id, x]));
  for (let x: Run | undefined = r; x; x = x.parent_id != null ? byId.get(x.parent_id) : undefined)
    if (x.name) return x.name;
  return null;
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
