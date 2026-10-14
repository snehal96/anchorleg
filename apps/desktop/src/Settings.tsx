// How runs pick an account (`anchorleg policy`) and what each CLI runs with (`anchorleg settings`).
import { useEffect, useState } from "react";
import { cliSettings, policy, type PolicyInfo, type VendorSettings } from "./api";

/** The CLIs anchorleg runs today; kimi and cursor aren't usable yet. */
const CLIS = [
  ["claude", "Claude Code", "low, medium, high, xhigh, max"],
  ["codex", "Codex", "low, medium, high, xhigh, max, ultra"],
  ["antigravity", "Antigravity", "low, medium, high, xhigh, max"],
] as const;

export default function Settings({ onChanged }: { onChanged: () => void }) {
  const [p, setP] = useState<PolicyInfo | null>(null);
  const [vendors, setVendors] = useState<Record<string, VendorSettings> | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    policy().then(setP, (e) => setError(String(e)));
    cliSettings().then((s) => setVendors(s.vendors), (e) => setError(String(e)));
  }, []);
  const run = async (f: () => Promise<void>) => {
    setError(null);
    try {
      await f();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="page">
      <div className="page-head">
        <h2>Settings</h2>
      </div>
      {error && <p className="error">{error}</p>}
      <section className="card">
        <h3>Switch policy</h3>
        <p className="muted small">How a run picks among accounts that have room.</p>
        {p?.policies.map((x) => (
          <label key={x.name} className="option">
            <input type="radio" name="policy" checked={p.policy === x.name} onChange={() => run(async () => setP(await policy(x.name)))} />
            <span>
              <b>{x.name}</b> <span className="muted">— {x.describe}</span>
            </span>
          </label>
        ))}
      </section>
      <section className="card">
        <h3>Per CLI</h3>
        <p className="muted small">Shared by every account of that CLI, so a task keeps the same model when it moves between accounts.</p>
        {vendors &&
          CLIS.map(([v, label, efforts]) => (
            <CliRow key={v} vendor={v} label={label} efforts={efforts} s={vendors[v] ?? { model: null, effort: null, args: [] }} run={run} onSaved={(s) => setVendors(s)} />
          ))}
      </section>
    </div>
  );
}

function CliRow({
  vendor,
  label,
  efforts,
  s,
  run,
  onSaved,
}: {
  vendor: string;
  label: string;
  efforts: string;
  s: VendorSettings;
  run: (f: () => Promise<void>) => void;
  onSaved: (v: Record<string, VendorSettings>) => void;
}) {
  const [model, setModel] = useState(s.model ?? "");
  const [effort, setEffort] = useState(s.effort ?? "");
  const [args, setArgs] = useState(s.args.join(" "));
  const dirty = model !== (s.model ?? "") || effort !== (s.effort ?? "") || args !== s.args.join(" ");
  const save = () =>
    run(async () => {
      const r = await cliSettings({
        vendor,
        model: model.trim() || "default",
        ...(efforts ? { effort: effort.trim() || "default" } : {}),
        args: args.trim(),
      });
      onSaved(r.vendors);
    });
  return (
    <div className="cli-row">
      <b className="cli-name">{label}</b>
      <input placeholder="model (CLI default)" value={model} onChange={(e) => setModel(e.target.value)} />
      {efforts ? <input placeholder="effort (CLI default)" title={`One of: ${efforts}`} value={effort} onChange={(e) => setEffort(e.target.value)} /> : <span />}
      <input className="mono grow" placeholder="extra arguments" value={args} onChange={(e) => setArgs(e.target.value)} />
      <button disabled={!dirty} onClick={save}>
        Save
      </button>
    </div>
  );
}
