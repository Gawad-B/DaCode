import { useState } from "react";
import {
  BLURBS,
  FLAT_KEYS,
  HERO,
  GRAMMAR,
  IDENTITY_CAPTIONS,
  MODAL_KEYS,
  MODES,
  MODE_HUE,
  NAME_BULLETS,
  PROJECT_NOTE,
  OVERHAUL,
  PRINCIPLES,
  RELEASES,
  SCOPE,
} from "./data";
import { useMode, usePrefersReducedMotion, useReveal, useScrolled } from "./hooks";
import { MiniEditor } from "./components/MiniEditor";
import { Download } from "./components/Download";
import { Screens } from "./components/Screens";
import { useRelease } from "./useRelease";
// The ض mark, built from the editor's own coverage mask
// (crates/dacode-render/assets/mark.png): ink in the editor's light grey, the
// block in the logo's amber, transparent around both so it sits on true black.
import mark from "./assets/mark.png";

export default function App() {
  const reduced = usePrefersReducedMotion();
  const { mode, setMode } = useMode(reduced);
  const scrolled = useScrolled();
  // Read rather than written in, so the release table stays the only place a version lives.
  const release = useRelease();
  const [grammar, setGrammar] = useState<"modal" | "flat">("modal");

  useReveal(reduced);

  const keys = grammar === "modal" ? MODAL_KEYS : FLAT_KEYS;

  // Counted from the table rather than written out, so that adding a release cannot
  // leave the sentence above the grid claiming a number that is no longer true.
  const shipped = RELEASES.filter((r) => r.state === "shipped" || r.state === "current").length;

  return (
    <div className="page">
      <header className={`nav ${scrolled ? "condensed" : ""}`}>
        <a href="#top" className="brand" aria-label="DaCode, back to top">
          <Logo size={30} />
          <Wordmark />
        </a>
        <span className="caret" />
        <span style={{ flex: 1 }} />
        {/* One link per section that exists, in the order they appear. */}
        <nav className="nav-links label">
          <a href="#why">why</a>
          <a href="#new">new</a>
          <a href="#editor">editor</a>
          <a href="#keys">keys</a>
          <a href="#releases">releases</a>
          <a href="#name">name</a>
          <a href="#identity">identity</a>
          <a href="#scope">scope</a>
        </nav>
      </header>

      {/* ── Hero ─────────────────────────────────────────────────────────── */}
      <section className="section hero" id="top">
        <div className="hero-left rule-v">
          <div className="hero-mark reveal">
            <Logo size={112} />
            <Wordmark style={{ fontSize: 40 }} />
          </div>
          <h1 className="headline reveal">
            A code editor that never <em>stalls</em> on your keystroke.
          </h1>
          <Bullets
            items={HERO.bullets}
            className="hero-blurb reveal"
            style={{ "--delay": "80ms" } as React.CSSProperties}
          />
          <aside
            className="project-note reveal"
            aria-label={PROJECT_NOTE.title}
            style={{ "--delay": "120ms" } as React.CSSProperties}
          >
            <span className="label">{PROJECT_NOTE.title}</span>
            <Bullets items={PROJECT_NOTE.bullets} />
          </aside>
          <div className="reveal" style={{ "--delay": "160ms" } as React.CSSProperties}>
            <Download reduced={reduced} />
          </div>
          <div className="specs reveal" style={{ "--delay": "240ms" } as React.CSSProperties}>
            {HERO.specs.map((s) => (
              <span key={s}>{s}</span>
            ))}
          </div>
        </div>

        <div className="mode-panel">
          <div className="mode-head">
            <span className="label">mode is the environment</span>
            <span className="mode-name">{mode}</span>
          </div>

          <div className="mode-buttons">
            {MODES.map((m) => (
              <button
                key={m}
                aria-pressed={m === mode}
                onClick={() => setMode(m)}
                style={
                  {
                    "--btn-hue": `oklch(var(--mode-l) var(--mode-c) ${MODE_HUE[m]})`,
                  } as React.CSSProperties
                }
              >
                {m}
              </button>
            ))}
          </div>

          <MiniEditor mode={mode} />

          <p className="prose" style={{ fontSize: 13.5, margin: 0, minHeight: "3.4em" }}>
            {BLURBS[mode]}
          </p>
          <span className="label" style={{ fontSize: 9.5 }}>
            click a mode, or press i · v · R · esc
          </span>
        </div>
      </section>

      {/* ── Three principles ─────────────────────────────────────────────── */}
      <section className="section triple" id="why">
        {PRINCIPLES.map((p, i) => (
          <div
            className={`principle reveal ${i < PRINCIPLES.length - 1 ? "rule-v" : ""}`}
            key={p.numeral}
            style={{ "--delay": `${i * 90}ms` } as React.CSSProperties}
          >
            <div className="numeral">{p.numeral}</div>
            <h3>{p.title}</h3>
            <Bullets items={p.bullets} />
          </div>
        ))}
      </section>

      {/* ── What the next release changes ────────────────────────────────── */}
      <section className="section overhaul" id="new">
        <div className="releases-head reveal">
          <h2>The interface, rebuilt</h2>
          <span className="prose" style={{ fontSize: 13, color: "var(--ink-faint)" }}>
            unreleased · on main now, in the next release; the download above is still {release.version}
          </span>
        </div>
        <RuledGrid>
          {OVERHAUL.map((f, i) => (
            <div
              className="card feature reveal"
              key={f.title}
              style={{ "--delay": `${(i % 3) * 70}ms` } as React.CSSProperties}
            >
              <h3 className="name">{f.title}</h3>
              <Bullets items={f.bullets} className="body" />
            </div>
          ))}
        </RuledGrid>
      </section>

      <Screens />

      {/* ── Grammar ──────────────────────────────────────────────────────── */}
      <section className="section split" id="keys">
        <div className="split-left alt rule-v">
          <h2 className="reveal">
            Vim's grammar.
            <br />
            Or none at all.
          </h2>
          <Bullets items={GRAMMAR} className="reveal" style={{ maxWidth: "46ch" }} />
        </div>

        <div className="split-right">
          <div className="toggle-row reveal">
            <button aria-pressed={grammar === "modal"} onClick={() => setGrammar("modal")}>
              vim keys on
            </button>
            <button aria-pressed={grammar === "flat"} onClick={() => setGrammar("flat")}>
              vim keys off
            </button>
          </div>
          <div className="keytable">
            {keys.map((k, i) => (
              <div
                className="keyrow reveal"
                key={`${grammar}-${k.keys}`}
                style={{ "--delay": `${i * 60}ms` } as React.CSSProperties}
              >
                <span className="keys">{k.keys}</span>
                <span className="gloss">{k.gloss}</span>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* ── Releases ─────────────────────────────────────────────────────── */}
      <section className="section releases" id="releases">
        <div className="releases-head reveal">
          <h2>What shipped</h2>
          <span className="prose" style={{ fontSize: 13, color: "var(--ink-faint)" }}>
            {shipped} releases in four days, then real use finding what review could not
          </span>
        </div>
        <RuledGrid>
          {RELEASES.map((r, i) => (
            <div
              className={`card reveal ${r.state === "shipped" ? "" : r.state}`}
              key={r.name}
              style={{ "--delay": `${(i % 3) * 70}ms` } as React.CSSProperties}
            >
              <span className="name">{r.name}</span>
              <span className="body">{r.body}</span>
            </div>
          ))}
        </RuledGrid>
      </section>

      {/* ── The name and the mark ─────────────────────────────────────────── */}
      <section className="section name" id="name">
        <div className="name-mark reveal">
          <Logo size={176} />
        </div>
        <div className="name-text">
          <h2 className="reveal">The name and the mark</h2>
          <ul className="bullets reveal">
            {NAME_BULLETS.map((b) => (
              <li key={b.text}>
                {b.text}
                {b.arabic && (
                  <>
                    {" "}
                    <span className="arabic" lang="ar" dir="rtl" title={b.arabicLabel}>
                      {b.arabic}
                    </span>
                  </>
                )}
              </li>
            ))}
          </ul>
        </div>
      </section>

      {/* ── Identity ─────────────────────────────────────────────────────── */}
      <section className="section identity" id="identity">
        <h2 className="reveal">Identity</h2>
        <div className="quad">
          <div className="spec reveal">
            <span className="label">mark</span>
            <div
              className="spec-box"
              style={{ alignItems: "center", justifyContent: "center", gap: 4 }}
            >
              <Logo size={52} />
              <Wordmark style={{ fontSize: 40, marginLeft: 10 }} />
              <span style={{ width: 9, height: 40, background: "var(--mode)" }} />
            </div>
            <span className="spec-caption">{IDENTITY_CAPTIONS.mark}</span>
          </div>

          <div className="spec reveal" style={{ "--delay": "80ms" } as React.CSSProperties}>
            <span className="label">mode palette</span>
            <div className="swatches">
              {MODES.map((m) => (
                <div
                  key={m}
                  title={m}
                  style={{ background: `oklch(var(--mode-l) var(--mode-c) ${MODE_HUE[m]})` }}
                >
                  {MODE_HUE[m]}
                </div>
              ))}
            </div>
            <span className="spec-caption">{IDENTITY_CAPTIONS.palette}</span>
          </div>

          <div className="spec reveal" style={{ "--delay": "160ms" } as React.CSSProperties}>
            <span className="label">cursor set</span>
            <div
              className="spec-box"
              style={{
                flexDirection: "column",
                justifyContent: "center",
                gap: 10,
                padding: "0 18px",
                fontSize: 12.5,
              }}
            >
              <CursorRow hue={250} width={11} border>
                normal · wide, underlined
              </CursorRow>
              <CursorRow hue={155} width={2} solid blink>
                insert · bar, blinking
              </CursorRow>
              <CursorRow hue={70} width={40} alpha={0.3}>
                visual · the whole set
              </CursorRow>
              <CursorRow hue={25} width={11} alpha={0.5}>
                replace · filled
              </CursorRow>
            </div>
            <span className="spec-caption">{IDENTITY_CAPTIONS.cursors}</span>
          </div>

          <div className="spec reveal" style={{ "--delay": "240ms" } as React.CSSProperties}>
            <span className="label">type</span>
            <div
              className="spec-box"
              style={{ flexDirection: "column", justifyContent: "center", gap: 6, padding: "0 18px" }}
            >
              <span style={{ fontSize: 27, letterSpacing: "-0.02em" }}>Fira Code</span>
              <span style={{ fontSize: 14, color: "var(--ink-dim)" }}>code, prose, headings</span>
              <span className="label" style={{ fontSize: 11, letterSpacing: "0.18em" }}>
                labels · regular only
              </span>
            </div>
            <span className="spec-caption">{IDENTITY_CAPTIONS.type}</span>
          </div>
        </div>
      </section>

      {/* ── Scope ────────────────────────────────────────────────────────── */}
      <section className="scope" id="scope">
        <div className="rule-v">
          <h2 className="reveal">{SCOPE.notTitle}</h2>
          <Bullets items={SCOPE.not} className="reveal" />
        </div>
        <div className="alt">
          <h2 className="reveal">{SCOPE.successTitle}</h2>
          <Bullets items={SCOPE.success} className="reveal" />
          <div className="stamp label reveal">
            <span />
            <span>{SCOPE.stamp}</span>
          </div>
        </div>
      </section>

      <footer className="footer">
        <Logo size={24} />
        <Wordmark style={{ fontSize: 20 }} />
        <span className="caret" style={{ height: 14, width: 6 }} />
        <span style={{ flex: 1 }} />
        {/* Everything that used to live here pointed into the repository,
            which is private — five links that rendered fine and 404'd. */}
        <a href="#releases">download</a>
        <a href="#keys">keys</a>
        <span className="label">mit licence · fira code under the ofl</span>
      </footer>
    </div>
  );
}

/** The ض mark. Decorative beside the wordmark, which already says the name. */
function Logo({ size }: { size: number }) {
  return (
    <img className="logo" src={mark} width={size} height={size} alt="" aria-hidden="true" />
  );
}

/** One fact per line: the page is bullets, never paragraphs. */
function Bullets({
  items,
  className,
  style,
}: {
  items: string[];
  className?: string;
  style?: React.CSSProperties;
}) {
  return (
    <ul className={`bullets ${className ?? ""}`} style={style}>
      {items.map((t) => (
        <li key={t}>{t}</li>
      ))}
    </ul>
  );
}

/** "Da" in the logo's amber, as the editor's title bar draws it. */
function Wordmark({ style }: { style?: React.CSSProperties }) {
  return (
    <span className="wordmark" style={style}>
      <span className="da">Da</span>Code
    </span>
  );
}

/*
 * The grid draws its rules as 1px gaps over a background, so a row that stops
 * short shows that background as a slab of rule colour rather than as nothing.
 * Filling the last row keeps it whole at any count — to a multiple of six,
 * because the layout drops to two columns on a narrow window and to one on a
 * phone.
 */
function RuledGrid({ children }: { children: React.ReactNode[] }) {
  const fillers = (6 - (children.length % 6)) % 6;
  return (
    <div className="ruled-grid">
      {children}
      {Array.from({ length: fillers }, (_, i) => (
        <div className="card filler" key={`filler-${i}`} aria-hidden="true" />
      ))}
    </div>
  );
}

function CursorRow({
  hue,
  width,
  alpha,
  solid,
  border,
  blink,
  children,
}: {
  hue: number;
  width: number;
  alpha?: number;
  solid?: boolean;
  border?: boolean;
  blink?: boolean;
  children: React.ReactNode;
}) {
  const full = `oklch(var(--mode-l) var(--mode-c) ${hue})`;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
      <span
        className={blink ? "caret blink" : undefined}
        style={{
          width,
          height: 17,
          background: solid ? full : `oklch(var(--mode-l) var(--mode-c) ${hue} / ${alpha ?? 0.32})`,
          borderBottom: border ? `2px solid ${full}` : undefined,
        }}
      />
      <span style={{ color: "var(--ink-dim)" }}>{children}</span>
    </div>
  );
}
