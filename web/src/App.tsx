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
  REPO_URL,
  SCOPE,
} from "./data";
import { useMode, usePrefersReducedMotion, useReveal, useScrolled } from "./hooks";
import { MiniEditor } from "./components/MiniEditor";
import { Download } from "./components/Download";
import { Screens } from "./components/Screens";
import { Coverage, ProofStrip, Versus } from "./components/Versus";
import { useRelease } from "./useRelease";
// The same PNG the docs and the editor tour use: a screenshot the editor rendered itself.
import editorShot from "../../docs/images/editor.png";
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
        {/* The four places a visitor goes; everything else is reached by scrolling. */}
        <nav className="nav-links label">
          <a href="#vs">benchmarks</a>
          <a href="#editor">editor</a>
          <a href="#keys">keys</a>
          <a href="#releases">releases</a>
        </nav>
        <a className="nav-cta label" href="#get">
          download
        </a>
      </header>

      {/* ── Hero: one promise, one action, the product itself ────────────── */}
      <section className="hero" id="top">
        <div className="hero-copy">
          <div className="hero-mark reveal">
            <Logo size={52} />
            <Wordmark style={{ fontSize: 26 }} />
          </div>
          <h1 className="headline reveal">
            A code editor that never <em>stalls</em> on your keystroke.
          </h1>
          <p className="hero-lede reveal" style={{ "--delay": "60ms" } as React.CSSProperties}>
            {HERO.lede}
          </p>
          <div className="hero-download reveal" style={{ "--delay": "120ms" } as React.CSSProperties}>
            <Download reduced={reduced} compact />
          </div>
          <p className="hero-note reveal" style={{ "--delay": "180ms" } as React.CSSProperties}>
            {PROJECT_NOTE.short} <a href="#about">More about the project</a>
          </p>
        </div>
        <div className="hero-shot reveal" style={{ "--delay": "100ms" } as React.CSSProperties}>
          <img
            src={editorShot}
            width={1440}
            height={900}
            alt="DaCode editing a Rust file in its default theme"
            decoding="async"
          />
        </div>
      </section>

      <ProofStrip />

      {/* ── Three principles ─────────────────────────────────────────────── */}
      <section className="section triple" id="why">
        {PRINCIPLES.map((p, i) => (
          <div
            className="principle reveal"
            key={p.numeral}
            style={{ "--delay": `${i * 90}ms` } as React.CSSProperties}
          >
            <div className="numeral">{p.numeral}</div>
            <h3>{p.title}</h3>
            <Bullets items={p.bullets} />
          </div>
        ))}
      </section>

      {/* ── Measured against VSCode, and how much of it DaCode does ────────── */}
      <Versus />
      <Coverage />

      <Screens />

      {/* ── What the current release added ───────────────────────────────── */}
      <section className="section overhaul" id="new">
        <div className="section-head reveal">
          <span className="eyebrow label">released {release.published}</span>
          <h2>New in {release.version}</h2>
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

      {/* ── Grammar, with the mode panel that shows what a mode is ────────── */}
      <section className="section split" id="keys">
        <div className="split-left">
          <div className="section-head reveal">
            <span className="eyebrow label">keys</span>
            <h2>Vim's grammar. Or none at all.</h2>
          </div>
          <Bullets items={GRAMMAR} className="reveal" style={{ maxWidth: "46ch" }} />
          <div className="mode-panel reveal">
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

      {/* ── Releases: what is current and next in full, the history folded ── */}
      <section className="section releases" id="releases">
        <div className="section-head reveal">
          <span className="eyebrow label">releases</span>
          <h2>What shipped, and what is next</h2>
          <p className="section-sub">
            {shipped} releases in four days, then real use finding what review could not.
          </p>
        </div>
        <div className="rel-top">
          <div className="rel-now">
            {RELEASES.filter((r) => r.state === "current" || r.state === "unreleased").map((r) => (
              <div className={`card reveal ${r.state}`} key={r.name}>
                <span className="name">{r.name}</span>
                <span className="body">{r.body}</span>
                {r.state === "current" && <a href="#new">what it added ↑</a>}
              </div>
            ))}
          </div>
          <div className="rel-next reveal">
            <span className="label">next</span>
            <ul className="rel-list">
              {RELEASES.filter((r) => r.state === "next").map((r) => (
                <li key={r.name}>
                  <span className="rel-name">{r.name.replace(/^next · /, "")}</span>
                  <span className="rel-body">{r.body}</span>
                </li>
              ))}
            </ul>
          </div>
        </div>
        <details className="rel-history reveal">
          <summary>All {RELEASES.filter((r) => r.state === "shipped").length} earlier releases</summary>
          <ul className="rel-list">
            {RELEASES.filter((r) => r.state === "shipped")
              .reverse()
              .map((r) => (
                <li key={r.name}>
                  <span className="rel-name">{r.name}</span>
                  <span className="rel-body">{r.body}</span>
                </li>
              ))}
          </ul>
        </details>
      </section>

      {/* ── The name, the mark and the identity, in one place ─────────────── */}
      <section className="section about-name" id="name">
        <div className="name-row">
          <div className="name-mark reveal">
            <Logo size={140} />
          </div>
          <div className="name-text">
            <div className="section-head reveal">
              <span className="eyebrow label">the name</span>
              <h2>The name and the mark</h2>
            </div>
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
        </div>

        <div className="quad" id="identity">
          <div className="spec reveal">
            <span className="label">mark</span>
            <div
              className="spec-box"
              style={{ alignItems: "center", justifyContent: "center", gap: 4 }}
            >
              <Logo size={44} />
              <Wordmark style={{ fontSize: 30, marginLeft: 8 }} />
              <span style={{ width: 8, height: 30, background: "var(--mode)" }} />
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
                gap: 8,
                padding: "0 16px",
                fontSize: 12,
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
              style={{ flexDirection: "column", justifyContent: "center", gap: 6, padding: "0 16px" }}
            >
              <span style={{ fontSize: 24, letterSpacing: "-0.02em" }}>Fira Code</span>
              <span style={{ fontSize: 13, color: "var(--ink-dim)" }}>code, prose, headings</span>
              <span className="label" style={{ fontSize: 10.5, letterSpacing: "0.18em" }}>
                labels · regular only
              </span>
            </div>
            <span className="spec-caption">{IDENTITY_CAPTIONS.type}</span>
          </div>
        </div>
      </section>

      {/* ── Scope, and who made what ─────────────────────────────────────── */}
      <section className="section scope" id="about">
        <div>
          <h2 className="reveal">{PROJECT_NOTE.title}</h2>
          <Bullets items={PROJECT_NOTE.bullets} className="reveal" />
        </div>
        <div id="scope">
          <h2 className="reveal">{SCOPE.notTitle}</h2>
          <Bullets items={SCOPE.not} className="reveal" />
        </div>
        <div>
          <h2 className="reveal">{SCOPE.successTitle}</h2>
          <Bullets items={SCOPE.success} className="reveal" />
          <div className="stamp label reveal">
            <span />
            <span>{SCOPE.stamp}</span>
          </div>
        </div>
      </section>

      {/* ── The closing call to action: every platform and package ───────── */}
      <section className="section get" id="get">
        <div className="section-head reveal">
          <span className="eyebrow label">download</span>
          <h2>Get DaCode</h2>
          <p className="section-sub">Linux and Windows, MIT licensed. It checks for updates itself.</p>
        </div>
        <div className="reveal">
          <Download reduced={reduced} />
        </div>
      </section>

      <footer className="footer">
        <Logo size={24} />
        <Wordmark style={{ fontSize: 20 }} />
        <span className="caret" style={{ height: 14, width: 6 }} />
        <span style={{ flex: 1 }} />
        <a href={`${REPO_URL}/releases/latest`}>download</a>
        <a href={REPO_URL}>source</a>
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
