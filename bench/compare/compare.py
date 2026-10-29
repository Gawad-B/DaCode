#!/usr/bin/env python3
"""DaCode against VSCode, measured from outside both, the same way for each.

    python3 bench/compare/compare.py            # writes web/src/benchmark.json

Both editors run on a private rootful Xwayland (display :9), so the harness can read pixels and
inject keys without touching the desktop, and neither editor is favoured by its own display path.
Every number is a black-box measurement of the process tree the editor starts; neither editor is
asked to report on itself, because only one of them could be trusted to report the same way.

What is measured, and where it stops:

- Startup: launch until the screen has stopped changing for SETTLE seconds with the file drawn.
  "First paint" is the first frame that differs from the empty server.
- Keystroke latency: XTEST key press until the pixels of the edited line change. That includes the
  editor, its GPU path and Xwayland's copy, and excludes the desktop compositor and the monitor,
  which are the same for both — so it is the editor's share, not glass-to-glass.
- Memory: PSS summed over every process in the editor's tree. PSS divides shared pages between the
  processes sharing them, so a forty-process Electron app is not counted forty times.
- Idle CPU: utime + stime over IDLE seconds with a file open and nothing happening.

VSCode runs on a throwaway profile with no extensions; the only settings changed are the ones that
would otherwise put noise on the screen being watched (caret blink, welcome page, recommendation
toasts, update checks). Neither editor runs a language server: the files are CSS, PATH is cut down for both, and
VSCode's built-in CSS server is disabled.
"""

import argparse
import json
import os
import platform
import random
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import numpy as np  # noqa: E402
from x11 import Display  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
DISPLAY = ":9"
W, H = 1600, 1000
SETTLE = 2.0
# A frame changing less than this share of the screen is a widget ticking, not startup.
VISUAL = 0.0025
# Small changes can go on for a while; after this long with only those, startup is over.
LINGER = 4.0
IDLE = 20.0
CLK_TCK = os.sysconf("SC_CLK_TCK")


# ---------------------------------------------------------------- processes


def process_tree(root_pid: int, marker: str | None = None) -> list[int]:
    """The root and every descendant, plus anything whose command line names `marker`.

    The marker is the profile directory: an Electron helper that re-parents itself is still found.
    """
    children: dict[int, list[int]] = {}
    marked = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
            ppid = int(stat[stat.rindex(")") + 2:].split()[1])
            children.setdefault(ppid, []).append(int(entry.name))
            if marker and marker in (entry / "cmdline").read_bytes().decode(errors="replace"):
                marked.append(int(entry.name))
        except (OSError, ValueError):
            continue
    found, stack = set(), [root_pid, *marked]
    while stack:
        pid = stack.pop()
        if pid in found:
            continue
        found.add(pid)
        stack.extend(children.get(pid, []))
    return sorted(p for p in found if Path(f"/proc/{p}").exists())


def pss_mb(pids: list[int]) -> float:
    total = 0
    for pid in pids:
        try:
            for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    total += int(line.split()[1])
        except OSError:
            pass
    return total / 1024


def cpu_ticks(pids: list[int]) -> int:
    total = 0
    for pid in pids:
        try:
            stat = Path(f"/proc/{pid}/stat").read_text()
            fields = stat[stat.rindex(")") + 2:].split()
            total += int(fields[11]) + int(fields[12])
        except (OSError, ValueError):
            pass
    return total


def names(pids: list[int]) -> list[str]:
    out = []
    for pid in pids:
        try:
            out.append(Path(f"/proc/{pid}/comm").read_text().strip())
        except OSError:
            pass
    return out


def kill_tree(proc: subprocess.Popen, marker: str | None):
    pids = process_tree(proc.pid, marker)
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in pids:
            try:
                os.kill(pid, sig)
            except ProcessLookupError:
                pass
        deadline = time.time() + 3
        while time.time() < deadline and any(Path(f"/proc/{p}").exists() for p in pids):
            time.sleep(0.05)
    proc.wait()


# ---------------------------------------------------------------- editors


class Editor:
    def __init__(self, key, label, version, install, launch, marker=None):
        self.key, self.label, self.version = key, label, version
        self.install, self.launch, self.marker = install, launch, marker


def base_env() -> dict:
    # The input method belongs to the desktop's display, not the private one: left set, both
    # editors hand every key to an ibus that is not there and nothing is ever typed.
    dropped = ("WAYLAND_DISPLAY", "XMODIFIERS", "GTK_IM_MODULE", "QT_IM_MODULE", "QT_IM_MODULES")
    env = {k: v for k, v in os.environ.items() if k not in dropped}
    env["DISPLAY"] = DISPLAY
    # No language server on either side: VSCode with no extensions has none to start.
    env["PATH"] = "/usr/bin:/bin"
    return env


def dacode(work: Path) -> Editor:
    binary = REPO / "target/release/dacode"
    cfg = work / "dacode-config"
    cfg.mkdir()
    (cfg / "init.lua").write_text('dacode.set { vim_keys = false, updates = "off" }\n')
    version = subprocess.run([binary, "--version"], capture_output=True, text=True).stdout.split()[-1]
    installed = shutil.which("dacode") or str(binary)

    def launch(file: Path):
        state = Path(tempfile.mkdtemp(dir=work, prefix="dacode-state-"))
        env = base_env() | {"DACODE_CONFIG_DIR": str(cfg), "DACODE_STATE_DIR": str(state)}
        return subprocess.Popen([binary, file], env=env, cwd=file.parent,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    return Editor("dacode", "DaCode", version, [installed], launch)


def vscode(work: Path) -> Editor:
    home = Path("/usr/share/code")
    profile, extensions = work / "vscode-profile", work / "vscode-extensions"
    (profile / "User").mkdir(parents=True)
    extensions.mkdir()
    (profile / "User/settings.json").write_text(json.dumps({
        "editor.cursorBlinking": "solid",
        "workbench.startupEditor": "none",
        "workbench.tips.enabled": False,
        "extensions.ignoreRecommendations": True,
        "telemetry.telemetryLevel": "off",
        "update.mode": "none",
        "window.restoreWindows": "none",
        "files.hotExit": "off",
        # A fresh profile opens the Copilot chat panel and starts copilot-runtime. DaCode has no
        # assistant, so VSCode is measured without one: lighter than it ships, never heavier.
        "chat.disableAIFeatures": True,
    }, indent=2))
    version = subprocess.run(["code", "--version"], capture_output=True, text=True).stdout.split()[0]

    def launch(file: Path):
        args = [home / "code", "--user-data-dir", profile, "--extensions-dir", extensions,
                "--ozone-platform=x11", "--new-window", "--skip-welcome", "--skip-release-notes",
                "--disable-workspace-trust",
                # DaCode has no CSS server, so VSCode's built-in one is switched off to match. That
                # takes a process away from VSCode's side, never adds one.
                "--disable-extension", "vscode.css-language-features", file]
        return subprocess.Popen(args, env=base_env(), cwd=file.parent, start_new_session=True,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    return Editor("vscode", "VSCode", version, [str(home)], launch, marker=str(profile))


# ---------------------------------------------------------------- measuring


def pixels_changed(a: bytes, b: bytes) -> int:
    pa = np.frombuffer(a, np.uint32)
    pb = np.frombuffer(b, np.uint32)
    return int(np.count_nonzero(pa != pb))


def watch_startup(d: Display, empty: bytes, deadline: float) -> tuple[float, float]:
    """(first paint, visually complete) in ms from now. Places the window as soon as it is mapped.

    Visually complete is the last frame that changed more than VISUAL of the screen before it went
    quiet. A threshold rather than "the last change at all", because both editors keep a few
    pixels moving after they are usable — DaCode its frame-time readout and the notification dot,
    VSCode its status-bar items — and neither of those is the editor still starting.
    """
    t0 = time.perf_counter()
    last, first, changed, big, placed = empty, None, t0, None, False
    while True:
        now = time.perf_counter()
        if not placed:
            for win, _, _ in d.viewable_windows():
                d.place(win, W, H)
                placed = True
        frame = d.grab(0, 0, W, H)
        if frame != last:
            if pixels_changed(frame, last) > VISUAL * W * H:
                big = now
            last, changed = frame, now
            if first is None:
                first = now
        if big is not None and (now - changed >= SETTLE or now - big >= LINGER):
            return (first - t0) * 1000, (big - t0) * 1000
        if now - t0 > deadline:
            raise TimeoutError("the screen never settled")


def wait_quiet(d: Display, region, quiet=0.3, limit=10.0):
    last, changed, t0 = d.grab(*region), time.perf_counter(), time.perf_counter()
    while time.perf_counter() - changed < quiet and time.perf_counter() - t0 < limit:
        frame = d.grab(*region)
        if frame != last:
            last, changed = frame, time.perf_counter()
    return last


def changed_rows(a: bytes, b: bytes) -> list[int]:
    stride = len(a) // H
    return [y for y in range(H) if a[y * stride:(y + 1) * stride] != b[y * stride:(y + 1) * stride]]


def keystroke_latency(d: Display, samples: int) -> list[float]:
    """Alternates deleting and retyping `x`, timing each until the edited line's pixels change."""
    x, bs, y = d.keycode("x"), d.keycode("BackSpace"), d.keycode("y")
    full = (0, 0, W, H)
    # Nothing manages windows on the private server, so nothing gives one the keyboard either.
    for win, _, _ in d.viewable_windows():
        d.focus(win)
    click_y = H // 2 + 150
    d.click(W // 2 + 200, click_y)
    time.sleep(0.5)
    # A first character makes the document dirty for good, so the loop never returns it to its
    # saved state and the tab's dirty marker never changes in the middle of a measurement. The
    # marker animates in, so the screen is given a whole second to stop moving first.
    d.key(y, True), d.key(y, False)
    before = wait_quiet(d, full, quiet=1.0)
    d.key(x, True), d.key(x, False)
    after = wait_quiet(d, full, quiet=1.0)
    rows = changed_rows(before, after)
    if not rows:
        raise RuntimeError("typing changed nothing on screen — the editor does not have the keys")
    # The status bar and the tab can change too; the line typed into is the run of changed rows
    # nearest the click that put the caret there.
    bands, band = [], [rows[0]]
    for r in rows[1:]:
        if r != band[-1] + 1:
            bands.append(band)
            band = []
        band.append(r)
    bands.append(band)
    band = min(bands, key=lambda b: abs((b[0] + b[-1]) / 2 - click_y))
    strip = (0, band[0], W, band[-1] - band[0] + 1)
    print(f"  watching rows {band[0]}-{band[-1]}", flush=True)

    times, missed, keys = [], 0, [bs, x]
    for i in range(samples):
        code = keys[i % 2]
        base = wait_quiet(d, strip, quiet=0.08)
        t0 = time.perf_counter()
        d.key(code, True)
        while d.grab(*strip) == base:
            if time.perf_counter() - t0 > 1.0:
                missed += 1
                break
        else:
            times.append((time.perf_counter() - t0) * 1000)
        d.key(code, False)
        time.sleep(random.uniform(0.03, 0.09))
    if missed:
        print(f"  {missed} keystrokes changed nothing within a second and were not counted")
    # Xwayland presents a hidden window on a one-second fallback timer, so a covered or minimised
    # :9 window turns every answer into ~1000 ms. Refuse the run rather than publish the timer.
    if missed > samples // 10 or (times and statistics.median(times) > 500):
        raise RuntimeError(
            "keystrokes are answering on Xwayland's one-second timer: keep the :9 window visible "
            "and uncovered on the desktop for the whole run")
    return times


def summarize(xs: list[float]) -> dict:
    xs = sorted(xs)
    return {
        "median": round(statistics.median(xs), 2),
        "p95": round(xs[min(len(xs) - 1, int(len(xs) * 0.95))], 2),
        "min": round(xs[0], 2),
        "n": len(xs),
    }


def du_mb(paths: list[str]) -> float:
    out = subprocess.run(["du", "-sbL", *paths], capture_output=True, text=True).stdout
    return sum(int(line.split()[0]) for line in out.splitlines()) / 1e6


def bench(ed: Editor, d: Display, fixtures: dict, runs: int, keys: int) -> dict:
    def log(*a):
        print(f"[{ed.key}]", *a, flush=True)

    log("warm-up launch (discarded)")
    p = ed.launch(fixtures["small"])
    watch_startup(d, d.grab(0, 0, W, H), 60)
    kill_tree(p, ed.marker)
    time.sleep(1)

    first, settled = [], []
    for i in range(runs):
        empty = d.grab(0, 0, W, H)
        p = ed.launch(fixtures["small"])
        f, s = watch_startup(d, empty, 60)
        first.append(f)
        settled.append(s)
        log(f"startup {i + 1}/{runs}: first paint {f:.0f} ms, visually complete {s:.0f} ms")
        if i < runs - 1:
            kill_tree(p, ed.marker)
            time.sleep(1)

    # The last startup stays open for the steady-state measurements.
    time.sleep(3)
    pids = process_tree(p.pid, ed.marker)
    idle_mem = statistics.median(pss_mb(process_tree(p.pid, ed.marker)) for _ in range(3))
    log(f"idle: {len(pids)} processes, {idle_mem:.0f} MB PSS — {sorted(set(names(pids)))}")
    t0, c0 = time.perf_counter(), cpu_ticks(pids)
    time.sleep(IDLE)
    pids = process_tree(p.pid, ed.marker)
    idle_cpu = (cpu_ticks(pids) - c0) / CLK_TCK / (time.perf_counter() - t0) * 100
    log(f"idle CPU {idle_cpu:.2f}% of a core")
    lat = keystroke_latency(d, keys)
    log(f"keystroke median {statistics.median(lat):.1f} ms over {len(lat)}")
    kill_tree(p, ed.marker)
    time.sleep(1)

    p = ed.launch(fixtures["big"])
    _, big_settled = watch_startup(d, d.grab(0, 0, W, H), 120)
    time.sleep(3)
    big_mem = pss_mb(process_tree(p.pid, ed.marker))
    log(f"100k-line file: settled {big_settled:.0f} ms, {big_mem:.0f} MB PSS")
    kill_tree(p, ed.marker)
    time.sleep(1)

    return {
        "label": ed.label,
        "version": ed.version,
        "startup_first_paint_ms": summarize(first),
        "startup_ready_ms": summarize(settled),
        "keystroke_ms": summarize(lat),
        "idle_memory_mb": round(idle_mem, 1),
        "idle_processes": len(pids),
        "idle_cpu_percent": round(idle_cpu, 2),
        "big_file_settled_ms": round(big_settled),
        "big_file_memory_mb": round(big_mem, 1),
        "install_mb": round(du_mb(ed.install), 1),
    }


def make_fixtures(work: Path) -> dict:
    project = work / "project"
    project.mkdir()
    # CSS because both editors highlight it and neither, as configured here, runs a server for it.
    # Rust was the first choice and was dropped: with no rust-analyzer, DaCode puts up a card saying
    # so, and timing a card fading out is not timing startup.
    sample = (REPO / "web/src/styles.css").read_text()
    small = project / "styles.css"
    small.write_text(sample)
    # A hundred thousand lines of real CSS, so both highlighters do the work they would do.
    lines = sample.splitlines()
    big = project / "big.css"
    big.write_text("\n".join(lines[i % len(lines)] for i in range(100_000)) + "\n")
    return {"small": small, "big": big}


def machine() -> dict:
    cpu = next((line.split(":", 1)[1].strip() for line in open("/proc/cpuinfo")
                if line.startswith("model name")), platform.processor())
    mem = int(next(line for line in open("/proc/meminfo") if line.startswith("MemTotal")).split()[1])
    xw = subprocess.run(["Xwayland", "-version"], capture_output=True, text=True)
    return {
        "cpu": cpu,
        "cores": os.cpu_count(),
        "ram_gb": round(mem / 1024 / 1024),
        "os": platform.freedesktop_os_release().get("PRETTY_NAME", platform.system()),
        "kernel": platform.release(),
        "display": (xw.stdout + xw.stderr).splitlines()[0].replace("The X.Org Foundation ", ""),
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--runs", type=int, default=10, help="startups per editor")
    ap.add_argument("--keys", type=int, default=200, help="timed keystrokes per editor")
    ap.add_argument("--only", choices=["dacode", "vscode"])
    ap.add_argument("--out", type=Path, default=REPO / "web/src/benchmark.json")
    args = ap.parse_args()

    xw = subprocess.Popen(["Xwayland", DISPLAY, "-geometry", f"{W}x{H}", "-noreset"],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(1.5)
        d = Display(DISPLAY)
        with tempfile.TemporaryDirectory(prefix="dacode-vs-vscode-") as tmp:
            work = Path(tmp)
            fixtures = make_fixtures(work)
            results = {}
            for ed in (dacode(work), vscode(work)):
                if args.only and ed.key != args.only:
                    continue
                results[ed.key] = bench(ed, d, fixtures, args.runs, args.keys)
    finally:
        xw.terminate()
        xw.wait()

    report = {
        "date": time.strftime("%Y-%m-%d"),
        "machine": machine(),
        "window": f"{W}x{H}",
        "method": {
            "runs": args.runs,
            "keystrokes": args.keys,
            "settle_seconds": SETTLE,
            "idle_seconds": IDLE,
            "big_file_lines": 100_000,
        },
        "editors": results,
    }
    args.out.write_text(json.dumps(report, indent=2) + "\n")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
