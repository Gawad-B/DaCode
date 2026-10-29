/*
 * DaCode against VSCode: the measurements and the feature list.
 *
 * The numbers are not written here. `benchmark.json` is the output of
 * `python3 bench/compare/compare.py`, and every figure and ratio on the page is
 * computed from it, so rerunning the harness is the only way to change them.
 *
 * The feature list is written by hand, and is meant to be the list a VSCode user
 * would check first — including the things DaCode does not have. A comparison
 * that leaves out the debugger is an advertisement, and readers can tell.
 */

import bench from "./benchmark.json";

export type Editor = {
  label: string;
  version: string;
  startup_first_paint_ms: Stat;
  startup_ready_ms: Stat;
  keystroke_ms: Stat;
  idle_memory_mb: number;
  idle_processes: number;
  idle_cpu_percent: number;
  big_file_settled_ms: number;
  big_file_memory_mb: number;
  install_mb: number;
};

type Stat = { median: number; p95: number; min: number; n: number };

export const BENCH = bench as {
  date: string;
  machine: { cpu: string; cores: number; ram_gb: number; os: string; kernel: string; display: string };
  window: string;
  method: { runs: number; keystrokes: number; big_file_lines: number };
  editors: { dacode: Editor; vscode: Editor };
};

export type Metric = {
  label: string;
  note: string;
  unit: "ms" | "MB" | "%";
  pick: (e: Editor) => number;
};

/** Lower is better for every one of these, which is what lets the bars share a rule. */
export const METRICS: Metric[] = [
  {
    label: "Startup",
    note: "launch to a highlighted file on screen · median",
    unit: "ms",
    pick: (e) => e.startup_ready_ms.median,
  },
  {
    label: "Keystroke",
    note: "key press to changed pixels · median",
    unit: "ms",
    pick: (e) => e.keystroke_ms.median,
  },
  {
    label: "Keystroke, slow end",
    note: "the 95th percentile — what a stutter feels like",
    unit: "ms",
    pick: (e) => e.keystroke_ms.p95,
  },
  {
    label: "Memory",
    note: "one file open, every process counted (PSS)",
    unit: "MB",
    pick: (e) => e.idle_memory_mb,
  },
  {
    label: "Memory, 100k-line file",
    note: "the same, with a hundred thousand lines open",
    unit: "MB",
    pick: (e) => e.big_file_memory_mb,
  },
  {
    label: "Open 100k lines",
    note: "launch to that file highlighted on screen",
    unit: "ms",
    pick: (e) => e.big_file_settled_ms,
  },
  {
    label: "Idle CPU",
    note: "share of one core, twenty seconds doing nothing",
    unit: "%",
    pick: (e) => e.idle_cpu_percent,
  },
  {
    label: "On disk",
    note: "the installed program",
    unit: "MB",
    pick: (e) => e.install_mb,
  },
];

export const METHOD = [
  "One machine, both editors, one after the other, nothing else running.",
  "Both run on a private Xwayland display, so the harness reads pixels and presses keys.",
  "Nothing is self-reported: times are pixels changing, memory is /proc.",
  "VSCode gets a fresh profile, no extensions, and AI features off.",
  "CSS files, so neither runs a language server; VSCode's built-in one is off.",
  "Keystroke latency excludes the desktop compositor and monitor, which are the same for both.",
];

export type Status = "yes" | "partial" | "no";

export const PARITY: { group: string; items: [string, Status, string?][] }[] = [
  {
    group: "Editing",
    items: [
      ["Multiple cursors, select next occurrence", "yes"],
      ["Find and replace, with regex", "yes"],
      ["Folding and sticky scroll", "yes"],
      ["Auto-closing pairs", "yes"],
      ["Snippets", "yes"],
      ["Word wrap and zoom", "yes"],
      ["Minimap", "no"],
      ["Bracket pair colours", "no", "jump to the matching bracket only"],
    ],
  },
  {
    group: "Navigation",
    items: [
      ["Command palette", "yes"],
      ["Quick open by file name", "yes"],
      ["Go to symbol, in the file and project", "yes"],
      ["Split editors and tabs", "yes"],
      ["Search and replace across the project", "yes"],
      ["Breadcrumbs", "partial", "the path, not the symbols"],
    ],
  },
  {
    group: "Language servers",
    items: [
      ["Completion, with auto-import", "yes"],
      ["Hover, definition, references", "yes"],
      ["Rename symbol", "yes"],
      ["Code actions", "yes"],
      ["Diagnostics and a problems list", "yes"],
      ["Inlay hints", "yes"],
      ["Format document, format on save", "yes"],
      ["Signature help", "no"],
      ["Highlighting for every language", "partial", "about ten, by tree-sitter"],
    ],
  },
  {
    group: "Git",
    items: [
      ["Stage, commit, branch, push, pull, stash", "yes"],
      ["Change markers and revert hunk", "yes"],
      ["Inline blame", "yes"],
      ["Diff view", "partial", "unified, not side by side"],
    ],
  },
  {
    group: "Workbench",
    items: [
      ["Integrated terminal", "partial", "tabs, no splits"],
      ["Settings and keybinding editors", "yes"],
      ["Themes, and importing VSCode's", "yes"],
      ["Image and PDF preview", "yes"],
      ["Settings sync", "no"],
      ["Multi-root workspaces", "no"],
    ],
  },
  {
    group: "Platform",
    items: [
      ["Debugger", "no"],
      ["Extension marketplace", "no"],
      ["Remote: SSH, containers, WSL", "no"],
      ["Notebooks", "no"],
      ["Test explorer", "no"],
      ["Emmet", "no"],
    ],
  },
];

/** A partial counts half: it does the job, just not all of it. */
export function coverage() {
  const all = PARITY.flatMap((g) => g.items);
  const yes = all.filter(([, s]) => s === "yes").length;
  const partial = all.filter(([, s]) => s === "partial").length;
  return { yes, partial, total: all.length, percent: Math.round(((yes + partial / 2) / all.length) * 100) };
}
