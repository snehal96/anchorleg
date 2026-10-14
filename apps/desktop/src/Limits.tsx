// The header's quota pill: the account the next run would use and how much room it has left.
// Hovering shows every account's windows; a click keeps it open with links to the full pages.
import { useEffect, useRef, useState } from "react";
import type { AccountStatus, Status } from "./api";
import { until, windowName } from "./Accounts";
import { Icon } from "./ui";
import { useVendors } from "./vendors";


/** Room left in an account's fullest window, 0–1, or null before any reading. */
export function roomLeft(a: AccountStatus): number | null {
  const used = a.windows.map((w) => w.used).filter((u): u is number => u != null);
  return used.length ? 1 - Math.max(...used) : null;
}

export function nextAccount(st: Status | null): AccountStatus | null {
  const n = st?.next;
  if (!st || !n || n.action === "no_accounts") return null;
  return st.accounts.find((a) => a.name === n.account) ?? null;
}

export default function Limits({
  st,
  refreshing,
  onRefresh,
  onPage,
}: {
  st: Status | null;
  refreshing: boolean;
  onRefresh: () => void;
  onPage: (p: "accounts" | "usage" | "settings") => void;
}) {
  const { label: vendorLabel } = useVendors();
  const [pinned, setPinned] = useState(false);
  const [hover, setHover] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    if (!pinned) return;
    const close = (e: MouseEvent) => ref.current && !ref.current.contains(e.target as Node) && setPinned(false);
    const key = (e: KeyboardEvent) => e.key === "Escape" && setPinned(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", key);
    };
  }, [pinned]);
  const next = nextAccount(st);
  const left = next ? roomLeft(next) : null;
  const waitUntil = st?.next.action === "wait" ? st.next.until : null;
  const level = left == null ? "unknown" : left <= 0.05 ? "full" : left <= 0.2 ? "high" : "ok";
  const label = !st
    ? "Limits"
    : st.next.action === "no_accounts"
      ? "No accounts"
      : waitUntil
        ? `All busy · ${until(waitUntil)}`
        : `${next?.name ?? ""}${left != null ? ` · ${Math.round(left * 100)}% left` : ""}`;
  const go = (p: "accounts" | "usage" | "settings") => {
    setPinned(false);
    setHover(false);
    onPage(p);
  };
  const open = pinned || hover;
  return (
    <div
      className="limits"
      ref={ref}
      onMouseEnter={() => (clearTimeout(timer.current), (timer.current = setTimeout(() => setHover(true), 150)))}
      onMouseLeave={() => (clearTimeout(timer.current), (timer.current = setTimeout(() => setHover(false), 200)))}
    >
      <button className={`limits-pill level-${level}`} onClick={() => setPinned(!pinned)} aria-expanded={open}>
        <Ring left={left} /> {label} <Icon name="chevron" size={12} />
      </button>
      {open && (
        <div className="limits-panel" role="dialog" aria-label="Limits">
          <div className="limits-head">
            <span>Room left on each account</span>
            <button className="icon-button" onClick={onRefresh} disabled={refreshing} aria-label="Check again" data-tip="Check again">
              <Icon name="refresh" size={14} />
            </button>
          </div>
          {!st?.accounts.length && <p className="muted small">No accounts yet.</p>}
          {st?.accounts.map((a) => (
            <div key={a.name} className={`limits-account ${a.name === next?.name ? "next" : ""}`}>
              <div className="limits-name">
                <b>{a.name}</b> <span className="muted">{vendorLabel(a.vendor)}</span>
                {a.name === next?.name && <span className="tag">next</span>}
              </div>
              {a.windows.length === 0 && <span className="muted small">Not read yet</span>}
              {a.windows.map((w) => {
                const l = w.used == null ? null : 1 - w.used;
                return (
                  <div key={w.window} className="limits-row" title={w.resets_at ? `Resets in ${until(w.resets_at)}` : undefined}>
                    <span className="limits-window">{windowName(w.window)}</span>
                    <span className="bar">
                      <span className={`fill ${l == null ? "" : l <= 0.05 ? "full" : l <= 0.2 ? "high" : "ok"}`} style={{ width: `${Math.round((l ?? 0) * 100)}%` }} />
                    </span>
                    <span className="limits-pct">{l == null ? "?" : `${Math.round(l * 100)}% left`}</span>
                  </div>
                );
              })}
            </div>
          ))}
          <div className="limits-links">
            <button className="link" onClick={() => go("accounts")}>
              <Icon name="people" size={14} /> Accounts
            </button>
            <button className="link" onClick={() => go("usage")}>
              <Icon name="chart" size={14} /> Usage
            </button>
            <button className="link" onClick={() => go("settings")}>
              <Icon name="gear" size={14} /> Settings
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/** A small ring: how much room is left. */
export function Ring({ left }: { left: number | null }) {
  const r = 6;
  const c = 2 * Math.PI * r;
  return (
    <svg className="ring" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="8" cy="8" r={r} className="ring-track" />
      {left != null && <circle cx="8" cy="8" r={r} className="ring-fill" strokeDasharray={`${c * left} ${c}`} transform="rotate(-90 8 8)" />}
    </svg>
  );
}
