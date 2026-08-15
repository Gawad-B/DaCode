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
  const [loaded, setLoaded] = useState<Partial<Record<ShotId, boolean>>>({});
  const shot = SHOTS.find((s) => s.id === active)!;

  return (
    <section className="section shots" id="editor">
      <div className="shots-head reveal">
        <h2>The editor itself</h2>
        <span className="prose" style={{ fontSize: 13, color: "var(--ink-faint)" }}>
          rendered offscreen by vex, not captured from a screen
        </span>
      </div>

      <div className="shot-tabs reveal">
        {SHOTS.map((s) => (
          <button
            key={s.id}
            aria-pressed={s.id === active}
            onClick={() => setActive(s.id)}
          >
            {s.label}
          </button>
        ))}
      </div>

      <div className="shot-frame reveal">
        <img
          key={active}
          src={SRC[active]}
          alt={`vex — ${shot.label}`}
          loading="lazy"
          decoding="async"
          className={loaded[active] ? "shown" : ""}
          onLoad={() => setLoaded((l) => ({ ...l, [active]: true }))}
        />
      </div>

      <p className="shot-caption reveal">{shot.caption}</p>
    </section>
  );
}
