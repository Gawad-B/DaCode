import { useState } from "react";
import { SHOTS, type ShotId } from "../data";

// Imported rather than copied into `public/`: these are the same PNGs the
// documentation uses, and two copies of a screenshot drift the moment one of
// them is regenerated.
import editor from "../../../docs/images/editor.png";
import commands from "../../../docs/images/commands.png";
import settings from "../../../docs/images/settings.png";
import tree from "../../../docs/images/tree.png";
import light from "../../../docs/images/light.png";

const SRC: Record<ShotId, string> = { editor, commands, settings, tree, light };

export function Screens() {
  const [active, setActive] = useState<ShotId>("editor");
  const shot = SHOTS.find((s) => s.id === active)!;

  return (
    <section className="section shots" id="editor">
      <div className="section-head reveal">
        <span className="eyebrow label">the editor</span>
        <h2>A tour, in five screens</h2>
        <p className="section-sub">Rendered offscreen by DaCode itself, not captured from a screen.</p>
      </div>

      <div className="shot-tabs reveal">
        {SHOTS.map((s) => (
          <button key={s.id} aria-pressed={s.id === active} onClick={() => setActive(s.id)}>
            {s.label}
          </button>
        ))}
      </div>

      {/*
        All five are in the DOM at once, stacked in a single grid cell, and
        switching changes which one is opaque. Swapping one <img>'s src meant
        a fetch and a decode on every press — the first switch to each tab
        stuttered, and the frame collapsed to nothing while it waited. They are
        all 1440x900, so the stack has one exact height and nothing reflows.
      */}
      <div className="shot-frame reveal">
        <div className="shot-stack">
          {SHOTS.map((s) => (
            <img
              key={s.id}
              src={SRC[s.id]}
              alt={s.id === active ? `DaCode — ${s.label}` : ""}
              width={1440}
              height={900}
              loading="lazy"
              decoding="async"
              aria-hidden={s.id !== active}
              className={s.id === active ? "shown" : ""}
            />
          ))}
        </div>
      </div>

      <ul className="bullets shot-caption reveal">
        {shot.bullets.map((b) => (
          <li key={b}>{b}</li>
        ))}
      </ul>
    </section>
  );
}
