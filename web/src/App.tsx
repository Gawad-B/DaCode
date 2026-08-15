import { useState } from "react";
import {
  BLURBS,
  FLAT_KEYS,
  HERO,
  IDENTITY_CAPTIONS,
  MODAL_KEYS,
  MODES,
  MODE_HUE,
  PRINCIPLES,
  RELEASES,
  SCOPE,
} from "./data";
import { useMode, usePrefersReducedMotion, useReveal, useScrolled, useTheme } from "./hooks";
import { MiniEditor } from "./components/MiniEditor";
import { Download } from "./components/Download";
import { Screens } from "./components/Screens";

export default function App() {
  const reduced = usePrefersReducedMotion();
  const { mode, setMode } = useMode(reduced);
  const { theme, toggle } = useTheme();
  const scrolled = useScrolled();
  const [grammar, setGrammar] = useState<"modal" | "flat">("modal");

  useReveal(reduced);

  const keys = grammar === "modal" ? MODAL_KEYS : FLAT_KEYS;

  // Counted from the table rather than written out, so that adding a release cannot
  // leave the sentence above the grid claiming a number that is no longer true.
  const shipped = RELEASES.filter((r) => r.state !== "next").length;
  const fillers = (6 - (RELEASES.length % 6)) % 6;

  return (
    <div className="page">
      <header className={`nav ${scrolled ? "condensed" : ""}`}>
        <span className="wordmark">vex</span>
        <span className="caret" />
        <span style={{ flex: 1 }} />
        {/* One link per section that exists, in the order they appear. */}
        <nav className="nav-links label">
          <a href="#why">why</a>
          <a href="#editor">editor</a>
          <a href="#keys">keys</a>
          <a href="#releases">releases</a>
          <a href="#identity">identity</a>
          <a href="#scope">scope</a>
          <button className="chip label" onClick={toggle}>
            {theme === "dark" ? "light" : "dark"}
          </button>
        </nav>
      </header>

      {/* ── Hero ─────────────────────────────────────────────────────────── */}
      <section className="section hero">
        <div className="hero-left rule-v">
          <h1 className="headline reveal" style={{ margin: 0, fontWeight: 400 }}>
            A code editor that never
            <br />
            <em>stalls</em> on your keystroke.
          </h1>
          <p className="prose hero-blurb reveal" style={{ "--delay": "80ms" } as React.CSSProperties}>
            {HERO.blurb}
          </p>
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

          <p className="prose" style={{ fontSize: 13.5, margin: 0, minHeight: "6.5em" }}>
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
            <div
              className="numeral"
              style={{ color: `oklch(var(--mode-l) var(--mode-c) ${p.hue})` }}
            >
              {p.numeral}
            </div>
            <h3>{p.title}</h3>
            <p className="prose">{p.body}</p>
          </div>
        ))}
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
          <p className="prose reveal" style={{ fontSize: 15, maxWidth: "42ch", margin: 0 }}>
            A cursor is a one-wide selection with a primary, so every operator maps over the set
            and multi-cursor is never a special case. Turn the Vim keys off and you get a real
            conventional editor rather than a modal one with the modes hidden — the keys you
            already know, because muscle memory is not something an editor gets to have opinions
            about.
          </p>
          <p className="prose reveal" style={{ fontSize: 13, maxWidth: "42ch", margin: 0 }}>
            Both columns come from one table in the source, which is also what the command
            palette, the F1 help panel and the menus read. A test presses every key in it through
            the real grammar, so a shortcut this page claims that did nothing would fail the
            build.
          </p>
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
        <div className="grid-9">
          {RELEASES.map((r, i) => (
            <div
              className={`card reveal ${r.state === "current" ? "current" : ""} ${
                r.state === "next" ? "next" : ""
              }`}
              key={r.name}
              style={{ "--delay": `${(i % 3) * 70}ms` } as React.CSSProperties}
            >
              <span className="name">{r.name}</span>
              <span className="body">{r.body}</span>
            </div>
          ))}
          {/* The grid draws its rules as 1px gaps over a background, so a row that
              stops short shows that background as a slab of rule colour rather than
              as nothing. It was exactly nine cards when it was written, which is what
              the name says; the tenth broke it. Filling the row keeps it whole at any
              count, and to a multiple of six because the layout drops to two columns
              on a narrow window. */}
          {Array.from({ length: fillers }, (_, i) => (
            <div className="card filler" key={`filler-${i}`} aria-hidden="true" />
          ))}
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
              <span
                className="wordmark"
                style={{ fontSize: 58 }}
              >
                vex
              </span>
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
              <span className="serif" style={{ fontSize: 27 }}>
                Instrument Serif
              </span>
              <span style={{ fontSize: 14, color: "var(--ink-dim)" }}>Fira Code</span>
              <span className="label" style={{ fontSize: 12, letterSpacing: "0.14em" }}>
                Plex Sans · labels
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
          <p className="prose reveal">{SCOPE.not}</p>
        </div>
        <div className="alt">
          <h2 className="reveal">{SCOPE.successTitle}</h2>
          <p className="prose reveal">{SCOPE.success}</p>
          <div className="stamp label reveal">
            <span />
            <span>{SCOPE.stamp}</span>
          </div>
        </div>
      </section>

      <footer className="footer">
        <span className="wordmark" style={{ fontSize: 20 }}>
          vex
        </span>
        <span className="caret" style={{ height: 14, width: 6 }} />
        <span style={{ flex: 1 }} />
        {/* Everything that used to live here pointed into the repository,
            which is private — five links that rendered fine and 404'd. */}
        <a href="#releases">download</a>
        <a href="#keys">keys</a>
        <span>mit licence</span>
      </footer>
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
