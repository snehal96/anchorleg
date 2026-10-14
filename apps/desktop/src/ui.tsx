// Small shared controls: icons, a dialog, a drop-down menu, a switch. Plain React, no library.
import { useEffect, useId, useRef, useState, type ReactNode } from "react";

const PATHS: Record<string, string> = {
  plus: "M12 5v14M5 12h14",
  stop: "M7 7h10v10H7z",
  send: "M4 12l16-8-6 16-2-7-8-1z",
  clip: "M21 11l-8.5 8.5a5 5 0 01-7-7L14 4a3.5 3.5 0 015 5l-8.5 8.5a2 2 0 01-3-3L15 7",
  file: "M14 3H6v18h12V7l-4-4zM14 3v4h4",
  terminal: "M4 6l6 6-6 6M12 18h8",
  diff: "M12 3v18M3 8h6M6 5v6M15 16h6",
  chat: "M4 5h16v11H8l-4 4V5z",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  folder: "M3 6h6l2 2h10v11H3z",
  check: "M5 12l5 5L20 7",
  x: "M6 6l12 12M18 6L6 18",
  undo: "M9 14L4 9l5-5M4 9h11a5 5 0 010 10h-3",
  review: "M11 4a7 7 0 100 14 7 7 0 000-14zM21 21l-5-5",
  fork: "M6 3v12M18 9a3 3 0 100-6 3 3 0 000 6zM6 21a3 3 0 100-6 3 3 0 000 6zM18 9c0 6-12 3-12 6",
  back: "M15 18l-6-6 6-6",
  goal: "M12 3a9 9 0 100 18 9 9 0 000-18zM12 8a4 4 0 100 8 4 4 0 000-8z",
  compact: "M4 9h16M4 15h16M9 4l3 3 3-3M9 20l3-3 3 3",
  pencil: "M4 20h4L19 9l-4-4L4 16v4z",
  shield: "M12 3l8 3v6c0 5-4 8-8 9-4-1-8-4-8-9V6l8-3z",
  sparkle: "M12 3l2 6 6 2-6 2-2 6-2-6-6-2 6-2 2-6z",
  chevron: "M6 9l6 6 6-6",
  refresh: "M20 11a8 8 0 10-2 6M20 4v7h-7",
  search: "M11 4a7 7 0 100 14 7 7 0 000-14zM21 21l-5-5",
  user: "M12 12a4 4 0 100-8 4 4 0 000 8zM4 21v-1a7 7 0 0114 0v1",
  gauge: "M12 14l3-3M3.5 18a9 9 0 1117 0",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  info: "M12 21a9 9 0 100-18 9 9 0 000 18zM12 11v5M12 8h.01",
  sliders: "M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0M14 4v4M8 10v4M16 16v4",
  tasks: "M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01",
  people: "M9 11a4 4 0 100-8 4 4 0 000 8zM2 21v-1a6 6 0 0112 0v1M16 3.5a4 4 0 010 7M22 21v-1a6 6 0 00-4-5.6",
  chart: "M4 20V10M10 20V4M16 20v-7M22 20H2",
  gear: "M12 15a3 3 0 100-6 3 3 0 000 6zM19.4 15a1.7 1.7 0 00.3 1.8l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.8-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.8.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.8 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.8l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.8.3H9a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.8-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.8V9a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z",
};

export function Icon({ name, size = 16 }: { name: keyof typeof PATHS | string; size?: number }) {
  return (
    <svg className="icon" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={PATHS[name] ?? ""} />
    </svg>
  );
}

/** A centred dialog over the window; Esc or a click outside closes it. */
export function Modal({ title, children, onClose, wide }: { title: string; children: ReactNode; onClose: () => void; wide?: boolean }) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);
  return (
    <div className="modal-back" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`modal ${wide ? "wide" : ""}`} role="dialog" aria-label={title}>
        <div className="modal-head">
          <h3>{title}</h3>
          <button className="icon-button" onClick={onClose} aria-label="Close">
            <Icon name="x" />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

export interface MenuItem {
  label: string;
  hint?: string;
  icon?: string;
  disabled?: boolean;
  danger?: boolean;
  onClick: () => void;
}

/** A button that opens a list of actions below it. */
export function Menu({ label, icon, items, align = "right", iconOnly }: { label: string; icon?: string; items: (MenuItem | "-")[]; align?: "left" | "right"; /** Just the icon; the label shows on hover. */ iconOnly?: boolean }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => ref.current && !ref.current.contains(e.target as Node) && setOpen(false);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);
  return (
    <div className="menu" ref={ref}>
      {iconOnly ? (
        <button className="icon-button" onClick={() => setOpen(!open)} aria-expanded={open} aria-label={label} data-tip={open ? undefined : label}>
          <Icon name={icon ?? "more"} />
        </button>
      ) : (
        <button className="secondary" onClick={() => setOpen(!open)} aria-expanded={open}>
          {icon && <Icon name={icon} />} {label} <Icon name="chevron" size={12} />
        </button>
      )}
      {open && (
        <ul className={`menu-list ${align}`} role="menu">
          {items.map((it, i) =>
            it === "-" ? (
              <li key={i} className="menu-sep" />
            ) : (
              <li key={i}>
                <button
                  role="menuitem"
                  className={it.danger ? "danger-text" : ""}
                  disabled={it.disabled}
                  onClick={() => {
                    setOpen(false);
                    it.onClick();
                  }}
                >
                  {it.icon && <Icon name={it.icon} />}
                  <span>
                    {it.label}
                    {it.hint && <span className="menu-hint">{it.hint}</span>}
                  </span>
                </button>
              </li>
            ),
          )}
        </ul>
      )}
    </div>
  );
}

/** An on/off switch with a label and an optional line of explanation. */
export function Switch({ checked, onChange, label, hint, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string; disabled?: boolean }) {
  return (
    <label className={`switch-row ${disabled ? "disabled" : ""}`}>
      <span className="switch">
        <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
        <span className="switch-track" />
      </span>
      <span>
        <span className="switch-label">{label}</span>
        {hint && <span className="field-hint">{hint}</span>}
      </span>
    </label>
  );
}

/** A labelled form field. A group, not a <label>: a label passes every click inside it to its
 *  first control, so a click anywhere in a field with buttons would press the first one. */
export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  const id = useId();
  return (
    <div className="field" role="group" aria-labelledby={id}>
      <span className="field-label" id={id}>
        {label}
      </span>
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </div>
  );
}

/** How tool calls are approved (`--mode`), in words anyone can pick from. */
export const APPROVAL: { value: string; label: string; hint: string }[] = [
  { value: "", label: "My usual setting", hint: "Whatever each agent is set up to do" },
  { value: "ask", label: "Ask me before changing anything", hint: "You approve every file change and command" },
  { value: "edits", label: "Let it edit files, ask before commands", hint: "File changes go ahead; commands wait for you" },
  { value: "auto", label: "Let the agent's own safety check decide", hint: "Fewer questions; risky steps are still stopped" },
];

export const STATE_WORDS: Record<string, string> = {
  running: "Working",
  waiting: "Waiting",
  switching: "Switching account",
  done: "Done",
  failed: "Failed",
  stopped: "Stopped",
};
