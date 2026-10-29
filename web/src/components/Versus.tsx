import { useState, type ReactNode } from "react";
import { BENCH, METHOD, METRICS, PARITY, coverage, type Editor, type Metric, type Status } from "../versus";
import { REPO_URL } from "../data";

/*
 * DaCode against VSCode, drawn. Every chart here is the emphasis form: DaCode in
 * the accent, VSCode in a recessive grey, one axis each, and a legend because
 * there are two series. The pair was checked with the dataviz palette validator
 * on the page's surface (#11a697 / #6b6b6b: CVD ΔE 12.2, normal 16.9, both over
 * 3:1); the grey's "chroma floor" failure is the point of emphasis, not a slip.
 *
 * Every value a tooltip shows is also printed beside its mark or in the table
 * under "every number", so hovering is never the only way to read one.
 */

const { dacode, vscode } = BENCH.editors;
const cpu = BENCH.machine.cpu.replace(/\((R|TM)\)/g, "").replace(/\s+@.*/, "");

const SERIES: { key: "dacode" | "vscode"; editor: Editor }[] = [
  { key: "dacode", editor: dacode },
  { key: "vscode", editor: vscode },
];

function seconds(ms: number) {
  return `${(ms / 1000).toFixed(2)} s`;
}

function format(value: number, unit: Metric["unit"]) {
  if (unit === "%") return `${value.toFixed(2)}%`;
  if (unit === "ms" && value >= 1000) return seconds(value);
  if (unit === "MB" && value >= 1000) return `${(value / 1000).toFixed(2)} GB`;
  const digits = value < 10 ? 1 : 0;
  return `${value.toLocaleString("en", { maximumFractionDigits: digits })} ${unit}`;
}

function times(ours: number, theirs: number) {
  const r = theirs / ours;
  return r >= 10 ? r.toFixed(0) : r.toFixed(1);
}

/** The top of an axis: the smallest round step past `max` that leaves four or five ticks. */
function axis(max: number) {
  const raw = max / 4;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw)!;
  const top = Math.ceil(max / step) * step;
  const ticks = Array.from({ length: Math.round(top / step) + 1 }, (_, i) => i * step);
  return { top, ticks };
}

// ── Tooltip ────────────────────────────────────────────────────────────────

type Tip = { x: number; y: number; value: string; label: string } | null;

/**
 * One tooltip for a chart. A mark binds itself with `hover(value, label)`, which
 * gives it pointer and keyboard handlers: the same readout on focus as on hover.
 */
function useTip() {
  const [tip, setTip] = useState<Tip>(null);
  const hover = (value: string, label: string) => ({
    tabIndex: 0,
    "aria-label": `${label}: ${value}`,
    onPointerMove: (e: React.PointerEvent) => setTip({ x: e.clientX, y: e.clientY, value, label }),
    onPointerLeave: () => setTip(null),
    onFocus: (e: React.FocusEvent<HTMLElement>) => {
      const r = e.currentTarget.getBoundingClientRect();
      setTip({ x: r.right, y: r.top + r.height / 2, value, label });
    },
    onBlur: () => setTip(null),
  });
  const node = tip && (
    <div className="vs-tip" style={{ left: tip.x + 14, top: tip.y + 14 }} role="presentation">
      <strong>{tip.value}</strong>
      <span>{tip.label}</span>
    </div>
  );
  return { hover, node };
}

function Legend() {
  return (
    <div className="vs-legend label" aria-hidden="true">
      <span className="vs-key dacode" /> dacode {dacode.version}
      <span className="vs-key vscode" /> vscode {vscode.version}
    </div>
  );
}

function ChartCard({
  title,
  sub,
  children,
  className,
}: {
  title: string;
  sub: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <figure className={`vs-card reveal ${className ?? ""}`}>
      <figcaption>
        <h3>{title}</h3>
        <p>{sub}</p>
      </figcaption>
      {children}
    </figure>
  );
}

/** The unit is printed once, on the last tick: "250 MB 500 MB 750 MB" crowds a phone-width axis. */
function Ticks({
  ticks,
  top,
  value,
  unit,
}: {
  ticks: number[];
  top: number;
  value: (v: number) => string;
  unit: string;
}) {
  const fmt = (t: number, i: number) => (i === ticks.length - 1 ? `${value(t)} ${unit}` : value(t));
  return (
    <>
      <div className="vs-grid-lines" aria-hidden="true">
        {ticks.map((t) => (
          <span key={t} style={{ left: `${(t / top) * 100}%` }} />
        ))}
      </div>
      <div className="vs-axis" aria-hidden="true">
        {ticks.map((t, i) => (
          <span key={t} style={{ left: `${(t / top) * 100}%` }}>
            {fmt(t, i)}
          </span>
        ))}
      </div>
    </>
  );
}

// ── The proof strip under the hero ───────────────────────────────────────────

export function ProofStrip() {
  const figures = [
    {
      value: times(dacode.startup_ready_ms.median, vscode.startup_ready_ms.median),
      what: "faster to a highlighted file",
      detail: `${seconds(dacode.startup_ready_ms.median)} against ${seconds(vscode.startup_ready_ms.median)}`,
    },
    {
      value: times(dacode.keystroke_ms.median, vscode.keystroke_ms.median),
      what: "quicker from key to pixels",
      detail: `${dacode.keystroke_ms.median.toFixed(1)} ms against ${vscode.keystroke_ms.median.toFixed(1)} ms`,
    },
    {
      value: times(dacode.idle_memory_mb, vscode.idle_memory_mb),
      what: "less memory with a file open",
      detail: `${Math.round(dacode.idle_memory_mb)} MB against ${Math.round(vscode.idle_memory_mb)} MB`,
    },
  ];
  return (
    <a className="proof" href="#vs" aria-label="Measured against VSCode — see the benchmarks">
      {figures.map((f, i) => (
        <div className="proof-item reveal" key={f.what} style={{ "--delay": `${i * 70}ms` } as React.CSSProperties}>
          <span className="proof-number">{f.value}×</span>
          <span className="proof-what">{f.what}</span>
          <span className="proof-detail">{f.detail}</span>
        </div>
      ))}
      <span className="proof-more label">vs VSCode {vscode.version} · how it was measured →</span>
    </a>
  );
}

// ── Charts ───────────────────────────────────────────────────────────────────

/** Two lanes on one time axis: each bar runs to "visually complete", a notch marks first paint. */
function StartupChart() {
  const { hover, node } = useTip();
  const { top, ticks } = axis(Math.max(vscode.startup_ready_ms.median, dacode.startup_ready_ms.median));
  const beatsWindow = dacode.startup_ready_ms.median < vscode.startup_first_paint_ms.median;
  return (
    <ChartCard
      className="wide"
      title="Startup"
      sub={
        <>
          Launch to the file on screen with its colours, median of {BENCH.method.runs} runs. The notch is
          the first frame drawn.
          {beatsWindow && " DaCode is finished before VSCode has drawn its first frame."}
        </>
      }
    >
      <div className="vs-plot">
        <Ticks ticks={ticks} top={top} value={(t) => `${t / 1000}`} unit="s" />
        {SERIES.map(({ key, editor }) => {
          const ready = editor.startup_ready_ms.median;
          const first = editor.startup_first_paint_ms.median;
          return (
            <div className="vs-lane" key={key}>
              <span className="vs-lane-name">{editor.label}</span>
              <div className="vs-track">
                <span
                  className={`vs-bar ${key}`}
                  style={{ width: `${(ready / top) * 100}%` }}
                  {...hover(seconds(ready), `${editor.label}, file highlighted`)}
                />
                <span
                  className="vs-notch"
                  style={{ left: `${(first / top) * 100}%` }}
                  {...hover(seconds(first), `${editor.label}, first frame`)}
                />
                <span className="vs-end" style={{ left: `${(ready / top) * 100}%` }}>
                  {seconds(ready)}
                </span>
              </div>
            </div>
          );
        })}
      </div>
      {node}
    </ChartCard>
  );
}

/** A range from the fastest keystroke to the 95th percentile, a dot at the median, and a frame. */
function KeystrokeChart() {
  const { hover, node } = useTip();
  const frame = 1000 / 60;
  const { top, ticks } = axis(Math.max(vscode.keystroke_ms.p95, dacode.keystroke_ms.p95, frame) * 1.05);
  const at = (v: number) => `${(v / top) * 100}%`;
  return (
    <ChartCard
      title="Keystroke"
      sub={`Key press to changed pixels, ${BENCH.method.keystrokes} keystrokes each. Line: fastest to 95th percentile; dot: median.`}
    >
      <div className="vs-plot has-ref">
        <Ticks ticks={ticks} top={top} value={(t) => `${t}`} unit="ms" />
        <div className="vs-overlay" aria-hidden="true">
          <div className="vs-ref" style={{ left: at(frame) }}>
            <span>one frame at 60 Hz</span>
          </div>
        </div>
        {SERIES.map(({ key, editor }) => {
          const k = editor.keystroke_ms;
          return (
            <div className="vs-lane" key={key}>
              <span className="vs-lane-name">{editor.label}</span>
              <div className="vs-track">
                <span
                  className={`vs-range ${key}`}
                  style={{ left: at(k.min), width: `calc(${at(k.p95)} - ${at(k.min)})` }}
                  {...hover(`${k.min.toFixed(1)}–${k.p95.toFixed(1)} ms`, `${editor.label}, fastest to 95th percentile`)}
                />
                <span
                  className={`vs-dot ${key}`}
                  style={{ left: at(k.median) }}
                  {...hover(`${k.median.toFixed(1)} ms`, `${editor.label}, median`)}
                />
                <span className="vs-end" style={{ left: at(k.p95) }}>
                  {k.median.toFixed(1)} ms
                </span>
              </div>
            </div>
          );
        })}
      </div>
      {node}
    </ChartCard>
  );
}

/** Grouped bars: the same two editors, a small file and a hundred-thousand-line one. */
function MemoryChart() {
  const { hover, node } = useTip();
  const groups = [
    { name: "one file", pick: (e: Editor) => e.idle_memory_mb },
    { name: `${BENCH.method.big_file_lines / 1000}k lines`, pick: (e: Editor) => e.big_file_memory_mb },
  ];
  const { top, ticks } = axis(Math.max(...groups.flatMap((g) => SERIES.map((s) => g.pick(s.editor)))));
  return (
    <ChartCard title="Memory" sub="Every process each editor starts, counted as PSS so shared pages are not counted twice.">
      <div className="vs-plot">
        <Ticks ticks={ticks} top={top} value={(t) => t.toLocaleString("en")} unit="MB" />
        {groups.map((g) => (
          <div className="vs-group" key={g.name}>
            <span className="vs-lane-name">{g.name}</span>
            <div className="vs-group-bars">
              {SERIES.map(({ key, editor }) => {
                const v = g.pick(editor);
                return (
                  <div className="vs-track thin" key={key}>
                    <span
                      className={`vs-bar ${key}`}
                      style={{ width: `${(v / top) * 100}%` }}
                      {...hover(`${Math.round(v)} MB`, `${editor.label}, ${g.name}`)}
                    />
                    <span className="vs-end" style={{ left: `${(v / top) * 100}%` }}>
                      {Math.round(v)} MB
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        ))}
      </div>
      {node}
    </ChartCard>
  );
}

function Tiles() {
  const tiles = [
    {
      label: "Idle CPU",
      ours: format(dacode.idle_cpu_percent, "%"),
      theirs: format(vscode.idle_cpu_percent, "%"),
      note: "of one core, doing nothing",
    },
    {
      label: "Processes",
      ours: `${dacode.idle_processes}`,
      theirs: `${vscode.idle_processes}`,
      note: "with one file open",
    },
    {
      label: "Open 100k lines",
      ours: seconds(dacode.big_file_settled_ms),
      theirs: seconds(vscode.big_file_settled_ms),
      note: "launch to highlighted",
    },
    {
      label: "On disk",
      ours: format(dacode.install_mb, "MB"),
      theirs: format(vscode.install_mb, "MB"),
      note: "the installed program",
    },
  ];
  return (
    <div className="vs-tiles">
      {tiles.map((t, i) => (
        <div className="vs-tile reveal" key={t.label} style={{ "--delay": `${i * 60}ms` } as React.CSSProperties}>
          <span className="label">{t.label}</span>
          <span className="vs-tile-value">
            <span className="vs-key dacode" aria-hidden="true" />
            {t.ours}
          </span>
          <span className="vs-tile-theirs">
            <span className="vs-key vscode" aria-hidden="true" />
            VSCode {t.theirs}
          </span>
          <span className="vs-tile-note">{t.note}</span>
        </div>
      ))}
    </div>
  );
}

// ── The section ──────────────────────────────────────────────────────────────

export function Versus() {
  return (
    <section className="section versus" id="vs">
      <div className="section-head reveal">
        <span className="eyebrow label">benchmarks</span>
        <h2>Measured against VSCode</h2>
        <p className="section-sub">
          Same machine, same files, measured from outside both editors — {cpu}, {BENCH.machine.ram_gb} GB, Linux,{" "}
          {BENCH.date}.
        </p>
        <Legend />
      </div>

      <div className="vs-charts">
        <StartupChart />
        <KeystrokeChart />
        <MemoryChart />
      </div>

      <Tiles />

      <div className="vs-more">
        <details className="reveal">
          <summary>Every number, as a table</summary>
          <table className="vs-table">
            <thead>
              <tr>
                <th scope="col">Measure</th>
                <th scope="col">DaCode</th>
                <th scope="col">VSCode</th>
                <th scope="col">DaCode's lead</th>
              </tr>
            </thead>
            <tbody>
              {METRICS.map((m) => {
                const ours = m.pick(dacode);
                const theirs = m.pick(vscode);
                return (
                  <tr key={m.label}>
                    <th scope="row">
                      {m.label}
                      <span>{m.note}</span>
                    </th>
                    <td>{format(ours, m.unit)}</td>
                    <td>{format(theirs, m.unit)}</td>
                    <td>{ours === 0 ? "none used" : `${times(ours, theirs)}× less`}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </details>
        <details className="reveal">
          <summary>How it was measured</summary>
          <ul className="bullets">
            {METHOD.map((t) => (
              <li key={t}>{t}</li>
            ))}
            <li>
              Rerun it yourself:{" "}
              <a href={`${REPO_URL}/blob/main/bench/compare/compare.py`}>bench/compare/compare.py</a>.
            </li>
          </ul>
        </details>
      </div>
    </section>
  );
}

// ── Coverage ─────────────────────────────────────────────────────────────────

const STATUS_LABEL: Record<Status, string> = { yes: "yes", partial: "in part", no: "missing" };

/**
 * One meter: yes, then partial, then the unfilled track, which is "missing".
 * One hue stepping darker (#14b8a6, #0f766e, track #164e4a), validated as an
 * ordinal ramp: absence reads as track, not as a third colour.
 */
function Meter({ yes, partial, total, hover }: {
  yes: number;
  partial: number;
  total: number;
  hover: ReturnType<typeof useTip>["hover"];
}) {
  const no = total - yes - partial;
  return (
    <div className="cov-meter">
      {yes > 0 && (
        <span className="cov-seg yes" style={{ flexGrow: yes }} {...hover(`${yes} of ${total}`, "fully there")} />
      )}
      {partial > 0 && (
        <span className="cov-seg partial" style={{ flexGrow: partial }} {...hover(`${partial} of ${total}`, "in part")} />
      )}
      {no > 0 && <span className="cov-seg no" style={{ flexGrow: no }} {...hover(`${no} of ${total}`, "missing")} />}
    </div>
  );
}

export function Coverage() {
  const cover = coverage();
  const { hover, node } = useTip();
  const missing = PARITY.flatMap((g) => g.items.filter(([, s]) => s === "no"));
  const partial = PARITY.flatMap((g) => g.items.filter(([, s]) => s === "partial"));

  return (
    <section className="section coverage" id="coverage">
      <div className="section-head reveal">
        <span className="eyebrow label">coverage</span>
        <h2>How much of VSCode it does</h2>
        <p className="section-sub">
          {cover.total} features a VSCode user reaches for first, checked against the code. The gaps are listed,
          not left out.
        </p>
      </div>

      <div className="cov-body">
        <div className="cov-chart reveal">
          <div className="cov-headline">
            <span className="cov-percent">{cover.percent}%</span>
            <span className="cov-of">
              {cover.yes} fully, {cover.partial} in part, {cover.total - cover.yes - cover.partial} missing
              <span className="cov-hint">a part counts half</span>
            </span>
          </div>
          <Meter yes={cover.yes} partial={cover.partial} total={cover.total} hover={hover} />
          <div className="cov-legend label" aria-hidden="true">
            <span className="cov-key yes" /> yes
            <span className="cov-key partial" /> in part
            <span className="cov-key no" /> missing
          </div>

          <div className="cov-groups">
            {PARITY.map((g) => {
              const yes = g.items.filter(([, s]) => s === "yes").length;
              const part = g.items.filter(([, s]) => s === "partial").length;
              return (
                <div className="cov-group" key={g.group}>
                  <span className="cov-group-name">{g.group}</span>
                  <Meter yes={yes} partial={part} total={g.items.length} hover={hover} />
                  <span className="cov-group-count">
                    {yes}/{g.items.length}
                  </span>
                </div>
              );
            })}
          </div>
        </div>

        <div className="cov-gaps reveal" style={{ "--delay": "80ms" } as React.CSSProperties}>
          <h3>Not in DaCode</h3>
          <ul className="cov-list">
            {missing.map(([text, , note]) => (
              <li key={text}>
                {text}
                {note && <span> — {note}</span>}
              </li>
            ))}
          </ul>
          <h3>Only in part</h3>
          <ul className="cov-list partial">
            {partial.map(([text, , note]) => (
              <li key={text}>
                {text}
                {note && <span> — {note}</span>}
              </li>
            ))}
          </ul>
        </div>
      </div>

      <details className="cov-all reveal">
        <summary>The full checklist</summary>
        <div className="cov-checklist">
          {PARITY.map((g) => (
            <div className="cov-check-group" key={g.group}>
              <h4>{g.group}</h4>
              <ul>
                {g.items.map(([text, status, note]) => (
                  <li key={text} className={status}>
                    <span className="cov-mark label">{STATUS_LABEL[status]}</span>
                    <span>
                      {text}
                      {note && <span className="cov-aside"> — {note}</span>}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </details>
      {node}
    </section>
  );
}
