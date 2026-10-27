import { useCallback, useEffect, useRef, useState } from "react";
import { MODES, MODE_HUE, type Mode } from "./data";

/** True when the visitor has asked their system for less motion. */
export function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() =>
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );

  useEffect(() => {
    const q = window.matchMedia("(prefers-reduced-motion: reduce)");
    const on = () => setReduced(q.matches);
    q.addEventListener("change", on);
    return () => q.removeEventListener("change", on);
  }, []);

  return reduced;
}

/*
 * Reveal-on-scroll. The observer unhooks each element the first time it lands,
 * because a section that fades out again on the way back up is a section the
 * visitor has to wait for twice.
 *
 * Two floors under it, because the hidden state is `opacity: 0` and anything
 * that keeps a section there keeps it *blank*:
 *
 *   1. The hidden state is applied by this hook (`.animate` on the root), never
 *      by the stylesheet. With scripting off, or if the bundle fails to load,
 *      the page is simply a page.
 *   2. A timer reveals whatever is left after SAFETY_MS. An IntersectionObserver
 *      does not fire in a tab that is not compositing — a background tab, a
 *      window the compositor has parked — and a visitor arriving at a page that
 *      was loaded in one is entitled to read it.
 */
const SAFETY_MS = 2000;

export function useReveal(reduced: boolean) {
  useEffect(() => {
    const root = document.documentElement;
    const all = () => Array.from(document.querySelectorAll<HTMLElement>(".reveal"));

    // Reveals everything at once and drops the transition with it. A tab that is
    // not compositing does not advance a transition either, so fading in from
    // zero there would land on exactly the blank page this is preventing.
    const revealEverything = () => {
      root.classList.add("settled");
      all().forEach((el) => el.classList.add("in"));
    };

    if (reduced) {
      revealEverything();
      return;
    }

    root.classList.add("animate");

    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            e.target.classList.add("in");
            io.unobserve(e.target);
          }
        }
      },
      { rootMargin: "0px 0px -12% 0px", threshold: 0.08 },
    );

    const observe = (el: HTMLElement) => {
      if (el.classList.contains("in")) return;
      // Once the fallback has run, the observer is known not to be firing, so
      // anything arriving after it must not be handed to the observer to wait
      // on — that is how the toggle would empty its section a second time.
      if (root.classList.contains("settled")) el.classList.add("in");
      else io.observe(el);
    };
    all().forEach(observe);

    /*
     * A `.reveal` that appears after mount — the grammar table swapping between
     * the Vim keys and the conventional ones — is hidden by the stylesheet the
     * instant it renders and has nothing watching it. That is not a hypothetical:
     * it shipped, and pressing the toggle emptied the section. Anything added
     * later is picked up here and, being already on screen, reveals immediately.
     */
    const mutations = new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (!(node instanceof HTMLElement)) continue;
          if (node.classList.contains("reveal")) observe(node);
          node.querySelectorAll<HTMLElement>(".reveal").forEach(observe);
        }
      }
    });
    mutations.observe(document.body, { childList: true, subtree: true });

    /*
     * The safety net fires only if the observer has revealed *nothing* by now,
     * which means it is not running at all. Revealing everything unconditionally
     * was wrong in the other direction: two seconds in, every section below the
     * fold was already marked, and the page scrolled with no animation left.
     */
    const safety = window.setTimeout(() => {
      if (document.querySelectorAll(".reveal.in").length === 0) revealEverything();
    }, SAFETY_MS);

    return () => {
      io.disconnect();
      mutations.disconnect();
      window.clearTimeout(safety);
      root.classList.remove("animate", "settled");
    };
  }, [reduced]);
}

const CYCLE_MS = 3400;

/*
 * The mode drives every accent on the page, so it lives on the document element
 * rather than in a React context — one attribute, and the CSS derives the rest
 * from a single hue. It cycles on its own until the visitor touches it, which
 * is how the idea lands without anyone having to read an instruction; the first
 * click or keypress stops it for good, because a control that keeps moving
 * after you have taken hold of it is a control that has been taken away.
 */
export function useMode(reduced: boolean) {
  const [mode, setModeState] = useState<Mode>("normal");
  const [live, setLive] = useState(true);
  const liveRef = useRef(true);

  const setMode = useCallback((next: Mode) => {
    liveRef.current = false;
    setLive(false);
    setModeState(next);
  }, []);

  useEffect(() => {
    document.documentElement.dataset.mode = mode;
  }, [mode]);

  useEffect(() => {
    if (reduced || !live) return;

    const id = window.setInterval(() => {
      if (liveRef.current) {
        setModeState((m) => MODES[(MODES.indexOf(m) + 1) % MODES.length]);
      }
    }, CYCLE_MS);

    return () => window.clearInterval(id);
  }, [reduced, live]);

  // i / v / R / Esc, the editor's own keys. A modifier held means the visitor is
  // talking to the browser, not to the page.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const target = e.target as HTMLElement | null;
      if (target && /^(input|textarea|select)$/i.test(target.tagName)) return;

      if (e.key === "Escape") setMode("normal");
      else if (e.key === "i") setMode("insert");
      else if (e.key === "v") setMode("visual");
      else if (e.key === "R") setMode("replace");
      else return;

      e.preventDefault();
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setMode]);

  return { mode, setMode, hue: MODE_HUE[mode], live };
}

/*
 * Types a string out one character at a time, once, when it first comes into
 * view. Retypes when the string itself changes — the install command is
 * per-platform, and a visitor switching tabs should see the new one arrive.
 */
export function useTypewriter(text: string, reduced: boolean, start: boolean) {
  const [shown, setShown] = useState(reduced || !start ? text : "");

  useEffect(() => {
    if (reduced || !start) {
      setShown(text);
      return;
    }

    setShown("");
    let i = 0;
    const id = window.setInterval(() => {
      i += 1;
      setShown(text.slice(0, i));
      if (i >= text.length) window.clearInterval(id);
    }, 26);

    return () => window.clearInterval(id);
  }, [text, reduced, start]);

  return shown;
}

/** True once the element has been on screen. Used to defer the typewriter. */
export function useInView<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [seen, setSeen] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setSeen(true);
          io.disconnect();
        }
      },
      { threshold: 0.2 },
    );

    io.observe(el);
    return () => io.disconnect();
  }, []);

  return { ref, seen };
}

/** Adds a class to the nav once the page has scrolled past the hero's top. */
export function useScrolled(px = 40) {
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const on = () => setScrolled(window.scrollY > px);
    on();
    window.addEventListener("scroll", on, { passive: true });
    return () => window.removeEventListener("scroll", on);
  }, [px]);

  return scrolled;
}
