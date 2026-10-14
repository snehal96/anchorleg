// Tokens and list-price cost of anchorleg's runs (`anchorleg usage --json`).
import { useEffect, useState } from "react";
import { usage, type Usage as UsageReport } from "./api";
import { short } from "./sessions";

const RANGES = [
  ["24h", "Last 24 hours"],
  ["7d", "Last 7 days"],
  ["30d", "Last 30 days"],
  ["all", "All time"],
] as const;

const money = (v: number | null | undefined) => (v == null ? "—" : `$${v.toFixed(v < 10 ? 2 : 0)}`);

export default function Usage() {
  const [since, setSince] = useState<string>("7d");
  const [report, setReport] = useState<UsageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setReport(null);
    usage(since).then(
      (r) => (setReport(r), setError(null)),
      (e) => setError(String(e)),
    );
  }, [since]);

  const max = Math.max(1, ...(report?.by_day.map((d) => d[1]) ?? []));
  const total = report?.totals.reduce((n, t) => n + (t.cost_usd ?? 0), 0) ?? 0;
  const tokens = report?.totals.reduce((n, t) => n + t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens, 0) ?? 0;
  return (
    <div className="page">
      <div className="page-head">
        <h2>Usage</h2>
        <select value={since} onChange={(e) => setSince(e.target.value)}>
          {RANGES.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
        <span className="spacer" />
        {report && (
          <span className="muted">
            {short(tokens)} tokens · {money(total)} at API prices
          </span>
        )}
      </div>
      <p className="muted small">
        What anchorleg's runs used, from what each CLI reported. Cost is the list price of the same tokens on the API: on a
        subscription you pay the plan, except what ran on credits.
      </p>
      {error && <p className="error">{error}</p>}
      {!report && !error && <p className="muted">Loading…</p>}
      {report && report.totals.length === 0 && <p className="muted">No runs reported usage in this period.</p>}
      {report && report.by_day.length > 0 && (
        <div className="chart" role="img" aria-label="Tokens per day">
          {report.by_day.map(([day, n, cost]) => (
            <div key={day} className="day" title={`${new Date(day * 1000).toLocaleDateString()}: ${short(n)} tokens, ${money(cost)}`}>
              <span className="col" style={{ height: `${(n / max) * 100}%` }} />
              <span className="day-label">{new Date(day * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" })}</span>
            </div>
          ))}
        </div>
      )}
      {report && report.totals.length > 0 && (
        <table className="table">
          <thead>
            <tr>
              <th>Account</th>
              <th>Model</th>
              <th className="r">Launches</th>
              <th className="r">Input</th>
              <th className="r">Output</th>
              <th className="r">Cache read</th>
              <th className="r">Cache write</th>
              <th className="r">API price</th>
              <th className="r">On credits</th>
            </tr>
          </thead>
          <tbody>
            {report.totals.map((t, i) => (
              <tr key={i}>
                <td>{t.account}</td>
                <td className="mono">{t.model ?? "—"}</td>
                <td className="r">{t.launches}</td>
                <td className="r">{short(t.input_tokens)}</td>
                <td className="r">{short(t.output_tokens)}</td>
                <td className="r">{short(t.cache_read_tokens)}</td>
                <td className="r">{short(t.cache_write_tokens)}</td>
                <td className="r">{money(t.cost_usd)}</td>
                <td className="r">{t.credits_cost_usd ? money(t.credits_cost_usd) : "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
