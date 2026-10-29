import { BENCH, METHOD, METRICS, PARITY, coverage, type Metric, type Status } from "../versus";
import { REPO_URL } from "../data";

const { dacode, vscode } = BENCH.editors;
const cpu = BENCH.machine.cpu.replace(/\((R|TM)\)/g, "").replace(/\s+@.*/, "");

function format(value: number, unit: Metric["unit"]) {
  if (unit === "%") return `${value.toFixed(2)}%`;
  if (unit === "ms" && value >= 1000) return `${(value / 1000).toFixed(2)} s`;
  const digits = value < 10 ? 1 : 0;
  return `${value.toLocaleString("en", { maximumFractionDigits: digits })} ${unit}`;
}

/**
 * How many times better DaCode is, said the right way round. A row where VSCode
 * wins says so rather than printing a fraction nobody reads as a loss.
 */
function verdict(ours: number, theirs: number) {
  if (ours === 0 && theirs === 0) return { text: "even", lead: false };
  if (ours === 0) return { text: "none at all", lead: true };
  const r = theirs / ours;
  const fmt = (x: number) => (x >= 10 ? x.toFixed(0) : x.toFixed(1));
  if (r >= 1.05) return { text: `${fmt(r)}× less`, lead: true };
  if (r <= 1 / 1.05) return { text: `VSCode ${fmt(1 / r)}× less`, lead: false };
  return { text: "even", lead: false };
}

function ratio(pick: (e: typeof dacode) => number) {
  return pick(dacode) > 0 ? pick(vscode) / pick(dacode) : 0;
}

const MARK: Record<Status, string> = { yes: "yes", partial: "part", no: "no" };

export function Versus() {
  const cover = coverage();
  const figures = [
    { value: ratio((e) => e.startup_ready_ms.median), what: "faster to a highlighted file" },
    { value: ratio((e) => e.keystroke_ms.median), what: "less time from key to pixels" },
    { value: ratio((e) => e.idle_memory_mb), what: "less memory with a file open" },
  ];

  return (
    <section className="section versus" id="vs">
      <div className="releases-head reveal">
        <h2>Against VSCode</h2>
        <span className="prose" style={{ fontSize: 13, color: "var(--ink-faint)" }}>
          DaCode {dacode.version} · VSCode {vscode.version} · measured {BENCH.date} on one {cpu}, {BENCH.machine.ram_gb} GB RAM, Linux
        </span>
      </div>

      <div className="vs-figures">
        {figures.map((f, i) => (
          <div
            className="vs-figure reveal"
            key={f.what}
            style={{ "--delay": `${i * 80}ms` } as React.CSSProperties}
          >
            <span className="vs-number">{f.value >= 10 ? f.value.toFixed(0) : f.value.toFixed(1)}×</span>
            <span className="prose">{f.what}</span>
          </div>
        ))}
      </div>

      <div className="vs-table" role="table" aria-label="DaCode and VSCode, measured">
        <div className="vs-legend label" aria-hidden="true">
          <span className="vs-key ours" /> dacode
          <span className="vs-key theirs" /> vscode
          <span style={{ flex: 1 }} />
          lower is better, every row
        </div>
        {METRICS.map((m, i) => {
          const ours = m.pick(dacode);
          const theirs = m.pick(vscode);
          const top = Math.max(ours, theirs) || 1;
          const v = verdict(ours, theirs);
          return (
            <div
              className="vs-row reveal"
              role="row"
              key={m.label}
              style={{ "--delay": `${(i % 4) * 50}ms` } as React.CSSProperties}
            >
              <div className="vs-what" role="rowheader">
                <span className="vs-label">{m.label}</span>
                <span className="vs-note">{m.note}</span>
              </div>
              <div className="vs-bars" role="cell">
                <Bar className="ours" share={ours / top} text={format(ours, m.unit)} />
                <Bar className="theirs" share={theirs / top} text={format(theirs, m.unit)} />
              </div>
              <span className={`vs-verdict ${v.lead ? "lead" : ""}`} role="cell">
                {v.text}
              </span>
            </div>
          );
        })}
      </div>

      <div className="vs-method reveal">
        <span className="label">how it was measured</span>
        <ul className="bullets">
          {METHOD.map((t) => (
            <li key={t}>{t}</li>
          ))}
          <li>
            {BENCH.method.runs} startups, {BENCH.method.keystrokes} keystrokes each; rerun it with{" "}
            <a href={`${REPO_URL}/blob/main/bench/compare/compare.py`}>bench/compare/compare.py</a>.
          </li>
        </ul>
      </div>

      <div className="releases-head reveal vs-parity-head">
        <h2>How much of VSCode it does</h2>
        <span className="prose" style={{ fontSize: 13, color: "var(--ink-faint)" }}>
          {cover.percent}% · {cover.yes} of {cover.total} everyday features, {cover.partial} in part — the
          gaps listed, not hidden
        </span>
      </div>
      <div className="ruled-grid">
        {PARITY.map((g, i) => (
          <div
            className="card feature reveal"
            key={g.group}
            style={{ "--delay": `${(i % 3) * 70}ms` } as React.CSSProperties}
          >
            <h3 className="name">{g.group}</h3>
            <ul className="vs-parity">
              {g.items.map(([text, status, note]) => (
                <li key={text} className={status}>
                  <span className="vs-mark label">{MARK[status]}</span>
                  <span>
                    {text}
                    {note && <span className="vs-aside"> — {note}</span>}
                  </span>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </section>
  );
}

function Bar({ className, share, text }: { className: string; share: number; text: string }) {
  return (
    <div className={`vs-bar ${className}`}>
      <span className="vs-track">
        <span className="vs-fill" style={{ width: `max(2px, ${(share * 100).toFixed(2)}%)` }} />
      </span>
      <span className="vs-value">{text}</span>
    </div>
  );
}
