import type { Mode } from "../data";

/*
 * Real lines from `crates/vex-text/src/anchor.rs`, at their real numbers — the
 * design's mock had a `version: Version` field that Anchor does not have. The
 * cursor sits on 25, and its shape is what changes with the mode: a wide
 * underlined block, a blinking bar, a selection, a filled block.
 */
export function MiniEditor({ mode }: { mode: Mode }) {
  return (
    <div className="mini" aria-hidden="true">
      <div className="mini-gutter">
        <div>22</div>
        <div>23</div>
        <div>24</div>
        <div className="here">25</div>
        <div>26</div>
      </div>
      <div className="mini-code">
        <div>
          #[<span className="fn">derive</span>(Debug, Copy, Clone, Eq, Hash)]
        </div>
        <div>
          <span className="kw">pub struct</span> <span className="ty">Anchor</span> {"{"}
        </div>
        <div>
          {"    "}
          <span className="kw">pub</span> offset: <span className="ty">CharOffset</span>,
        </div>
        <div className="active">
          {"    "}
          {mode === "visual" ? (
            <>
              <span className="sel">
                <span className="kw">pub</span> bias: <span className="ty">Bias</span>,
              </span>
              <span className="cur" />
            </>
          ) : (
            <>
              <span className="kw">pub</span> bias: <span className="ty">Bias</span>,
              <span className={`cur ${mode}`} />
            </>
          )}
        </div>
        <div>{"}"}</div>
      </div>
    </div>
  );
}
