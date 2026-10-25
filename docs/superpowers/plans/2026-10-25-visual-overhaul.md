# Visual Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the two reported bugs (colours waiting on a keypress, arrows captured by the tree), make
the touchpad scroll, add a spring-based motion engine, put an always-visible menu bar in the title
bar, restyle every overlay as one card style, and replace the settings panel with an auto-saving
settings tab.

**Architecture:** Background workers wake winit's event loop through an `EventLoopProxy` instead of
being polled. Keyboard routing reads an explicit `Focus`. Animation is one `Motion` store of springs
keyed by a small enum, with time passed in, so the chrome stays a pure function of state and "now".
Every new clickable rectangle comes from a geometry function both drawing and the mouse read.

**Tech Stack:** Rust 2024, winit 0.30, wgpu 30, glyphon 0.12, mlua (config), cargo test / clippy.

**Spec:** `docs/superpowers/specs/2026-10-25-visual-overhaul-design.md`

## Global Constraints

- Nothing unbounded on the UI thread; per-frame work is bounded by the viewport (`CLAUDE.md`, "The one rule").
- `cargo clippy --workspace --all-targets` stays at zero warnings; `cargo fmt --all` before each commit.
- Every new user-visible operation is an `Action` + `Action::from_name` + `Action::NAMES` + a row in `dacode_input::commands::COMMANDS`.
- A config option exists in all three places: `crates/dacode-lua/src/lib.rs`, `crates/dacode/src/config.rs`, `crates/dacode/src/settings_ui.rs`.
- Every mouse-reachable rectangle comes from one geometry function read by both drawing and hit-testing.
- Typing, the cursor and selections are never animated.
- Test names are sentences; tests that guard a rule are verified by injecting the violation once.
- Visual work is verified with `DACODE_NO_SYSTEM_FONTS=1 ./target/debug/dacode --screenshot …` at more than one `--size`.
- Commit messages are prose and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never stage `Cargo.toml` (the user's uncommitted debug-profile change), `AGENTS.md` or `DaCode logo.pdf`.
- Identifiers American (`color`), prose British-ish ("colour").

## File Map

| File | Responsibility | Tasks |
|---|---|---|
| `crates/dacode-syntax/src/worker.rs` | `SyntaxWorker::spawn_with(language, waker)`, `pending()` | 1 |
| `crates/dacode/src/wake.rs` (new) | `Waker` type, `Wake` user event | 1, 2 |
| `crates/dacode/src/main.rs` | event loop with user events, idle collection, focus routing, motion wiring, scroll input, menu bar clicks, settings tab | 1–3, 5–8, 11 |
| `crates/dacode/src/git.rs`, `format.rs`, `install.rs`, `crates/dacode-lsp/src/client.rs` | wake on answer | 2 |
| `crates/dacode/src/focus.rs` (new) | `Focus` enum and region → focus mapping | 3 |
| `crates/dacode/src/motion.rs` (new) | springs, `Motion`, `Key`, `Animations` | 4–7 |
| `crates/dacode/src/scroll.rs` (new) | fractional scroll accumulator | 6 |
| `crates/dacode/src/chrome.rs` | drawing: animated overlays, card style, menu bar, settings tab | 7–11 |
| `crates/dacode/src/layout.rs` | title-bar menu rects, animated sidebar width, `CodeGrid` sub-row | 6–8 |
| `crates/dacode/src/menu.rs` | menu bar reads `layout::title_bar` menus | 8 |
| `crates/dacode/src/settings_ui.rs` | settings page model, categories, search, explanations, page geometry | 10, 11 |
| `docs/…`, `CLAUDE.md`, `CHANGELOG.md`, `PLAN.md` | documentation | 12 |

---

### Task 1: Syntax colours arrive without a keypress

**Files:**
- Create: `crates/dacode/src/wake.rs`
- Modify: `crates/dacode/src/lib.rs` (add `pub mod wake;`)
- Modify: `crates/dacode-syntax/src/worker.rs:76` (`spawn` delegates to new `spawn_with`)
- Modify: `crates/dacode/src/main.rs` — event loop construction (`:142`), `impl ApplicationHandler for App` (`:1366`), `about_to_wait`, the tail of `sync_syntax` (`~:3686`), both `SyntaxWorker::spawn` sites (`:1464`, `:7273`), `services_busy` (`:3236`), the `wiring_tests::settle` helper.

**Interfaces:**
- Produces: `dacode::wake::Waker = std::sync::Arc<dyn Fn() + Send + Sync>`; `dacode::wake::Wake` (unit struct, the winit user event); `dacode_syntax::SyntaxWorker::spawn_with(language: Language, waker: Option<std::sync::Arc<dyn Fn() + Send + Sync>>) -> Result<SyntaxWorker, String>`; `SyntaxWorker::pending(&self) -> bool`; `App::collect_syntax(&mut self) -> bool` (applies a fresh result, returns whether highlights changed); `App.waker: Option<dacode::wake::Waker>`.

- [ ] **Step 1: Failing worker test** in `crates/dacode-syntax/src/worker.rs`'s tests:

```rust
#[test]
fn a_finished_parse_wakes_whoever_is_waiting() {
    // The editor sleeps until something wakes it. A parse that finishes silently is a parse whose
    // colours wait for the next keypress — the bug this exists for.
    use std::sync::atomic::{AtomicUsize, Ordering};
    let woken = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = woken.clone();
    let mut worker = SyntaxWorker::spawn_with(
        Language::Rust,
        Some(std::sync::Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })),
    )
    .unwrap();
    worker.request(request_for("fn main() {}\n", 1)); // build it as this file's other tests do
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while woken.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(woken.load(Ordering::SeqCst) > 0, "the waker was never called");
    assert!(worker.poll().is_some());
    assert!(!worker.pending(), "nothing is outstanding once the answer is collected");
}
```

`request_for(text, version)` is a small helper in the test module returning the `SyntaxRequest` literal the existing tests in this file already construct.

- [ ] **Step 2:** `cargo test -p dacode-syntax a_finished_parse` — expect a compile failure (`spawn_with` missing).
- [ ] **Step 3: Implement.** In `worker.rs`: move `waker: Option<Arc<dyn Fn() + Send + Sync>>` into the thread; after each `results.send(result)` call `if let Some(wake) = &waker { wake() }`. `pub fn spawn(language) -> Result<Self, String> { Self::spawn_with(language, None) }`. Track the newest requested version in `request` (`self.requested = Some(version)`) and in `pending` return `self.requested.is_some_and(|wanted| self.latest.as_ref().map(|r| r.version) != Some(wanted))`.
- [ ] **Step 4:** Create `wake.rs`:

```rust
//! How background workers tell the event loop they have an answer.
//!
//! The loop sleeps when there is no input. A worker that finishes while it sleeps must wake it, or
//! its answer waits for the next keypress — which is exactly how syntax colours came to appear only
//! after a click.

/// Called from a worker thread when a result is ready. Cheap and non-blocking.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// The user event the waker sends.
#[derive(Debug, Clone, Copy)]
pub struct Wake;
```

- [ ] **Step 5: Wire it.** `EventLoop::<dacode::wake::Wake>::with_user_event().build().context("creating the event loop")?`; before `run_app`, `let proxy = event_loop.create_proxy(); app.waker = Some(std::sync::Arc::new(move || { let _ = proxy.send_event(dacode::wake::Wake); }));`. `impl ApplicationHandler<dacode::wake::Wake> for App` with `fn user_event(&mut self, _: &ActiveEventLoop, _: dacode::wake::Wake) { if self.collect_syntax() { self.request_redraw(); } }`. Both `SyntaxWorker::spawn(language)` sites become `SyntaxWorker::spawn_with(language, self.waker.clone())`. Move the "poll and apply what is fresh" tail of `sync_syntax` into `fn collect_syntax(&mut self) -> bool` (returns true when it replaced `highlights`), call it at the end of `sync_syntax` and in `about_to_wait` beside `poll_walk`. In `services_busy`, the final clause becomes `self.waker.is_none() && self.syntax.as_ref().is_some_and(|worker| worker.pending())` — headless tests poll; the real editor is woken.
- [ ] **Step 6: Failing wiring test.** In `wiring_tests::settle`, add `app.collect_syntax();` to the loop body (it stands for the idle step). Then:

```rust
/// Colours arrive from the idle step alone — no key, no click. They used to wait for one.
#[test]
fn highlights_arrive_without_any_input() {
    let dir = scratch("idle-colours");
    let mut app = editor(&dir, "a.rs", "fn main() { let x = 1; }\n");
    app.sync_syntax();
    assert!(settle(&mut app, |app| !app.state.highlights.is_empty()));
}
```

- [ ] **Step 7:** Inject the bug once (remove `collect_syntax` from `settle`'s loop) → the test fails; restore → it passes.
- [ ] **Step 8:** `cargo test --workspace --no-fail-fast`, `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
- [ ] **Step 9: Commit** — "Wake the event loop when a parse finishes, so colours appear without a keypress".

### Task 2: Every background worker wakes the loop; the 100 ms poll goes

**Files:** `crates/dacode/src/git.rs:178`, `crates/dacode/src/format.rs:310`, `crates/dacode/src/install.rs:75`, `crates/dacode-lsp/src/client.rs` (reader thread, `LspClient::start`), `crates/dacode/src/main.rs` (`App::new` worker construction `:1773`, `:1807`; `LspClient::start` call `:2459`; `services_busy`; `user_event`; every `App::new` call in tests).

**Interfaces:**
- Consumes: `dacode::wake::Waker` (Task 1).
- Produces: `GitWorker::spawn_with(Option<Waker>)`, `FormatWorker::spawn_with(Option<Waker>)`, `Installer::spawn_with(Option<Waker>)` (each existing `spawn()` delegates with `None`); `dacode_lsp::LspClient::start_with(executable, program, arguments, root, waker: Option<std::sync::Arc<dyn Fn() + Send + Sync>>)` (`start` delegates with `None`); `App::new(root, state, grammar, settings, waker: Option<Waker>)`; `App::collect_background(&mut self) -> bool` (runs `collect_syntax`, `sync_lsp`, `poll_formatter`, `poll_installer`, `sync_git`; true if any reported a change).

- [ ] **Step 1: Failing tests,** one per worker, the shape of Task 1's: spawn with a counting waker, give it work, assert the counter rose and `poll()` returns `Some`. Git: a diff request on the temp repository `git.rs`'s existing tests create. Format: a request for a language with no formatter installed (it answers "unavailable"). Install: a request naming an unknown server (it answers an error). LSP: in `crates/dacode-lsp/tests`, guarded by `dacode_lsp::can_start("rust-analyzer")` like the existing live tests — start with a counting waker, wait for `initialize`'s reply, assert the counter rose.
- [ ] **Step 2:** Run them; expect compile failures.
- [ ] **Step 3:** Thread the waker into each worker thread and call it after every send on its results channel; in `client.rs` after the reader thread pushes any message.
- [ ] **Step 4:** `App::new` gains the `waker` parameter and builds workers with `spawn_with(waker.clone())`; `main` passes `app.waker` (move the proxy creation before `App::new`); tests pass `None`. `user_event` calls `collect_background()` and redraws when it returns true.
- [ ] **Step 5:** In `services_busy`, when `self.waker.is_some()`, return true only for the file walk and a streaming search (neither has a waker); the language-server, git, format and install clauses apply only when `self.waker.is_none()`.
- [ ] **Step 6:** `cargo test --workspace --no-fail-fast` (the live rust-analyzer tests run the LSP path), clippy, fmt.
- [ ] **Step 7: Commit** — "Let every worker wake the loop, so an idle editor with a language server sleeps".

### Task 3: Focus — keys go where you clicked

**Files:**
- Create: `crates/dacode/src/focus.rs`; add `pub mod focus;` to `lib.rs`.
- Modify: `crates/dacode/src/chrome.rs` (`EditorState` gains `pub focus: crate::focus::Focus`); every `EditorState` literal (`main.rs`, `tests/latency.rs`, `tests/editing_session.rs`, `benches/editor.rs`) gains `focus: Default::default()`.
- Modify: `crates/dacode/src/main.rs` (`handle_key` `:2043`; the tree/settings dispatch at `:2082-2087`; `click`; `show_tree`; anything closing the sidebar).
- Modify: `crates/dacode-input/src/{action.rs,commands.rs,grammar.rs}`, `crates/dacode/src/execute.rs` (two actions).

**Interfaces:**
- Produces: `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub enum Focus { #[default] Editor, Sidebar, Panel }`; `pub fn focus_for(layout: &crate::layout::Layout, x: f32, y: f32) -> Option<Focus>`; actions `Action::FocusExplorer` (name `"focus_explorer"`, flat key `<c-s-e>`, group View) and `Action::FocusEditor` (`"focus_editor"`, flat key `<c-1>`, group View).

- [ ] **Step 1: Failing tests** — in `focus.rs`:

```rust
#[test]
fn a_click_decides_where_the_keys_go() {
    let layout = crate::layout::Layout::of(Rect::new(0.0, 0.0, 1400.0, 900.0), 1.0, Some(4), true);
    let sidebar = layout.sidebar.expect("asked for one");
    assert_eq!(focus_for(&layout, sidebar.x + 5.0, sidebar.y + 50.0), Some(Focus::Sidebar));
    assert_eq!(focus_for(&layout, layout.editor.x + 50.0, layout.editor.y + 50.0), Some(Focus::Editor));
    let results = layout.results.expect("asked for one");
    assert_eq!(focus_for(&layout, results.x + 5.0, results.y + 5.0), Some(Focus::Panel));
    assert_eq!(
        focus_for(&layout, layout.title.x + 5.0, layout.title.y + 5.0),
        None,
        "the title bar leaves focus where it was"
    );
}
```

and in `main.rs` `wiring_tests` (use the key-dispatch entry the existing wiring tests use to press a key; `Key::new(KeyCode::Down)` stands for whatever constructor `dacode_input::Key` provides for an unmodified key):

```rust
/// With the tree open and the editor focused, ↓ moves the caret — it used to move the tree.
#[test]
fn arrows_follow_focus_not_whichever_panel_is_open() {
    let dir = scratch("focus");
    let mut app = editor(&dir, "a.rs", "one\ntwo\nthree\n");
    app.state.modal = false;
    app.grammar.set_modal(false);
    app.show_tree();
    app.state.focus = dacode::focus::Focus::Editor;
    app.press(Key::new(KeyCode::Down));
    assert_eq!(app.state.cursor_point().line, 1, "the caret moved");

    app.state.focus = dacode::focus::Focus::Sidebar;
    let before = app.state.cursor_point();
    app.press(Key::new(KeyCode::Down));
    assert_eq!(app.state.cursor_point(), before, "the tree took it");
}
```

If there is no `App::press(Key)` yet, add it in this task: it is the body of `handle_key` after the winit event has become a `Key` (split `handle_key` so the test drives the real routing).
- [ ] **Step 2:** Run; expect failures.
- [ ] **Step 3:** Implement `focus_for` (`Sidebar` inside `layout.sidebar`, `Panel` inside `layout.results`, `Editor` inside `layout.editor`, `None` elsewhere).
- [ ] **Step 4:** In the dispatch, call `handle_settings_key` and `handle_tree_key` only when `self.state.focus == Focus::Sidebar`; `esc` while `Sidebar` sets `Editor` and is consumed. `show_tree` and the activity bar opening a view set `Sidebar`; `close_sidebar` sets `Editor`. In `click`, after the welcome and menu branches: `if let Some(focus) = dacode::focus::focus_for(&layout, x, y) { self.state.focus = focus; }`.
- [ ] **Step 5:** Add the two actions in the four places; `execute` sets focus (`FocusExplorer` returns `Outcome::Window` when the tree is closed so the app opens it first); `feed_flat` binds `<c-s-e>` and `<c-1>`.
- [ ] **Step 6: Visual cue** in `chrome.rs::draw_explorer`: the selected row gets `theme.row_active` and a 1 px `theme.primary` outline (`outline_rounded`) when the sidebar has focus, `theme.rule_soft` and no outline when it does not.
- [ ] **Step 7:** Inject: drop the focus check before `handle_tree_key` → the wiring test fails; restore.
- [ ] **Step 8:** Full tests, clippy, fmt; render `--tree` (the screenshot path leaves focus on the editor) and read it.
- [ ] **Step 9: Commit** — "Route keys by focus, so arrows move lines while the tree is open".

### Task 4: The motion engine

**Files:** Create `crates/dacode/src/motion.rs`; add `pub mod motion;` to `lib.rs`.

**Interfaces — produces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Animations { #[default] Full, Reduced, Off }
impl Animations { pub fn name(self) -> &'static str; pub fn from_name(name: &str) -> Option<Self>; pub const ALL: [Animations; 3]; }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring { pub stiffness: f32, pub damping_ratio: f32 }
impl Spring { pub const PANEL: Spring; pub const POPUP: Spring; pub const HOVER: Spring; pub const SCROLL: Spring; }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key { Sidebar, Palette, Menu, Completion, Info, Help, Notifications, Hover(u32), TabIndicator, ActivityIndicator, Scroll(usize), ScrollX(usize), Bell, Welcome, SettingsCategory, Glow(u32) }

#[derive(Debug, Clone, Default)]
pub struct Motion { /* HashMap<Key, Value>, animations: Animations */ }
impl Motion {
    pub fn new(animations: Animations) -> Self;
    pub fn set_animations(&mut self, animations: Animations);
    pub fn to(&mut self, key: Key, target: f32, spring: Spring, now: Instant);  // retarget, keeping velocity
    pub fn snap(&mut self, key: Key, value: f32);                               // jump, no animation
    pub fn get(&self, key: Key, now: Instant) -> f32;                           // 0.0 for a key never set
    pub fn target(&self, key: Key) -> Option<f32>;
    pub fn busy(&self, now: Instant) -> bool;
}
```

- [ ] **Step 1: Failing tests** in `motion.rs`:

```rust
use std::time::{Duration, Instant};

fn at(start: Instant, ms: u64) -> Instant {
    start + Duration::from_millis(ms)
}

fn peak(motion: &Motion, key: Key, start: Instant) -> f32 {
    (0..400).map(|step| motion.get(key, at(start, step * 5))).fold(f32::MIN, f32::max)
}

#[test]
fn a_spring_arrives_and_then_the_editor_can_sleep() {
    let start = Instant::now();
    let mut motion = Motion::new(Animations::Full);
    motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
    assert!(motion.busy(at(start, 16)));
    assert!((motion.get(Key::Sidebar, at(start, 2000)) - 1.0).abs() < 1e-3);
    assert!(!motion.busy(at(start, 2000)), "settled means no more frames are asked for");
}

#[test]
fn the_expressive_spring_overshoots_a_little_and_the_scroll_spring_never_does() {
    let start = Instant::now();
    let mut motion = Motion::new(Animations::Full);
    motion.to(Key::Palette, 1.0, Spring::PANEL, start);
    motion.to(Key::Scroll(0), 100.0, Spring::SCROLL, start);
    let overshoot = peak(&motion, Key::Palette, start);
    assert!(overshoot > 1.01 && overshoot < 1.15, "overshoot {overshoot}");
    assert!(
        peak(&motion, Key::Scroll(0), start) <= 100.0 + 1e-3,
        "text must not bounce past where it was scrolled to"
    );
}

#[test]
fn retargeting_mid_flight_keeps_momentum() {
    let start = Instant::now();
    let mut motion = Motion::new(Animations::Full);
    motion.to(Key::Sidebar, 1.0, Spring::PANEL, start);
    let mid = at(start, 60);
    let before = motion.get(Key::Sidebar, mid);
    motion.to(Key::Sidebar, 0.0, Spring::PANEL, mid);
    assert!(
        motion.get(Key::Sidebar, at(start, 70)) > before,
        "still travelling outward a moment after being sent back"
    );
}

#[test]
fn off_jumps_and_reduced_never_overshoots() {
    let start = Instant::now();
    let mut off = Motion::new(Animations::Off);
    off.to(Key::Sidebar, 1.0, Spring::PANEL, start);
    assert_eq!(off.get(Key::Sidebar, start), 1.0);
    assert!(!off.busy(start));

    let mut reduced = Motion::new(Animations::Reduced);
    reduced.to(Key::Palette, 1.0, Spring::PANEL, start);
    assert!(peak(&reduced, Key::Palette, start) <= 1.0 + 1e-3);
}
```

- [ ] **Step 2:** `cargo test -p dacode --lib motion` — fails (module missing).
- [ ] **Step 3: Implement.** Each value stores `{ position, velocity, target, spring, since: Instant }`. `get` evaluates the damped spring **analytically** from `since` to `now` — under-damped (`ζ < 1`), critically damped (`ζ = 1`) and over-damped closed forms — so `get` is pure and any instant can be rendered. `to` first evaluates position and velocity at `now`, then stores them with the new target and `since = now`. Settled when `|position − target| < 1e-3 · max(1, |target|)` and `|velocity| < 1e-2 · max(1, |target|)`. Presets: `PANEL { stiffness: 180.0, damping_ratio: 0.72 }`, `POPUP { 260.0, 0.78 }`, `HOVER { 400.0, 1.0 }`, `SCROLL { 320.0, 1.0 }`. `Reduced` uses `max(ratio, 1.0)`; `Off` makes `to` behave as `snap`.
- [ ] **Step 4:** Tests pass; add a proptest: for any targets in ±1e4 and any sequence of up to 8 retargets at increasing times, every `get` is finite and the value settles within 3 s of the last retarget.
- [ ] **Step 5:** clippy, fmt. **Commit** — "Add a spring-based motion engine, with time as a parameter".

### Task 5: Wire motion into the editor; the `animations` setting

**Files:** `crates/dacode/src/chrome.rs` (`Chrome` gains `pub motion: crate::motion::Motion` and `pub now: Option<std::time::Instant>`); `crates/dacode/src/main.rs` (`about_to_wait`, `redraw`, `FLAGS`/`VALUE_FLAGS`, screenshot path); `crates/dacode-lua/src/lib.rs`; `crates/dacode/src/config.rs`; `crates/dacode/src/settings_ui.rs`; `docs/reference/settings.md`; `docs/reference/cli.md`.

**Interfaces:**
- Consumes: Task 4.
- Produces: `Chrome.motion`, `Chrome.now` and `EditorState::now(&self) -> Instant` (`chrome.now.unwrap_or_else(Instant::now)`); config `animations = "full" | "reduced" | "off"` as `Config.animations: Option<String>`, `Settings.animations: Animations`, `Values.animations: Animations`, `Field::Animations` (a Choice); capture flag `--at-ms N`.

- [ ] **Step 1: Failing tests:** Lua `dacode.set { animations = "reduced" }` → `config.animations == Some("reduced".into())`, an unknown value is a problem in `config.problems`; `config::load`'s Lua application sets `Settings.animations`; `Values::cycle(Field::Animations)` goes full → reduced → off → full and `write_back` writes `animations = "reduced"`; a wiring test: after `app.state.chrome.motion.to(Key::Sidebar, 1.0, Spring::PANEL, now)` the idle step asks for a redraw (`app.animating(now)` returns true), and at `now + 2 s` it does not.
- [ ] **Step 2:** Implement the three config places (the pattern `transparency` followed in commit `df79cd6`). `App::animating(&self, now) -> bool` is `self.state.chrome.motion.busy(now)`; `about_to_wait` requests a redraw while it is true — winit paces redraws to vsync, so the loop turns once per frame only while something moves and sleeps after. `redraw` sets `self.state.chrome.now = Some(Instant::now())` before drawing.
- [ ] **Step 3:** `--at-ms N` in the screenshot path: record `let t0 = Instant::now()`, start the animations the requested view starts at `t0` (Task 7 adds them), and set `state.chrome.now = Some(t0 + Duration::from_millis(N))` before drawing. Add to `FLAGS` and `VALUE_FLAGS`.
- [ ] **Step 4:** Full tests, clippy, fmt. **Commit** — "Drive redraws from the motion engine while anything moves; add the animations setting".

### Task 6: Smooth scrolling and the touchpad

**Files:** Create `crates/dacode/src/scroll.rs` (`pub mod scroll;` in `lib.rs`); modify `crates/dacode/src/main.rs` (`WindowEvent::MouseWheel` at `:1631`, `scroll_by`, `wheel`; add `WindowEvent::PinchGesture`), `crates/dacode/src/layout.rs` (`CodeGrid`), `crates/dacode/src/chrome.rs` (`draw_code`), `crates/dacode/tests/latency.rs`.

**Interfaces:**
- Produces: `#[derive(Debug, Clone, Copy, Default)] pub struct ScrollInput { pending_px: f32 }` with `pub fn add_pixels(&mut self, px: f32, row_height: f32) -> isize` (whole rows to move now; keeps the remainder) and `pub fn remainder(&self) -> f32`; `CodeGrid::with_sub_row(self, px: f32) -> Self` and `CodeGrid::sub_row(&self) -> f32`; `App.scroll_input: ScrollInput`.

- [ ] **Step 1: Failing tests** in `scroll.rs`:

```rust
#[test]
fn many_small_touchpad_deltas_add_up_to_rows() {
    // The bug: each 3-pixel delta was divided by 40 and truncated to 0, so a touchpad scrolled nothing.
    let mut input = ScrollInput::default();
    let rows: isize = (0..40).map(|_| input.add_pixels(3.0, 26.0)).sum();
    assert_eq!(rows, 4, "120 px is four 26 px rows");
    assert!((input.remainder() - 16.0).abs() < 1e-3);
}

#[test]
fn reversing_direction_spends_the_remainder_first() {
    let mut input = ScrollInput::default();
    input.add_pixels(20.0, 26.0);
    assert_eq!(input.add_pixels(-30.0, 26.0), 0, "-10 px net is less than a row");
    assert!((input.remainder() + 10.0).abs() < 1e-3);
}
```

and in `layout.rs` tests:

```rust
#[test]
fn a_part_scrolled_row_is_still_the_row_under_the_pointer() {
    let code = Rect::new(0.0, 100.0, 800.0, 400.0);
    let grid = CodeGrid::new(code, 8.0, 26.0, 0, 1.0).with_sub_row(13.0);
    assert_eq!(grid.row_at(code.y), 0, "the half-hidden top row");
    assert_eq!(grid.row_at(code.y + 13.0 + 1.0), 1);
    assert_eq!(grid.row_at(code.y + 13.0 + 26.0 + 1.0), 2);
}
```

(Positive scrolling moves content up; `sub_row` is how far the top row has moved up out of view. `row_at(y) = floor((y − code.y + sub_row) / row_height)`; row *n* is drawn at `code.y + n·row_height − sub_row`.)
- [ ] **Step 2:** Implement. `PixelDelta` → `add_pixels(-dy)` and `scroll_by` the whole rows returned (touchpads already animate themselves); the remainder is the sub-row. `LineDelta` → `motion.to(Key::Scroll(pane), current + lines·row_height, Spring::SCROLL, now)`; `about_to_wait`, while that key is busy, converts the spring's travel since the last frame into `add_pixels` calls, so wheel scrolling glides and lands on whole rows. Horizontal deltas use the same path into `scroll.column` (`Key::ScrollX`) when wrap is off. `draw_code` offsets rows by `−sub_row` and draws one extra row; all quads and runs stay clipped to `code`. Pinch: accumulate `WindowEvent::PinchGesture { delta, .. }` and run `IncreaseFontSize`/`DecreaseFontSize` per ±0.1.
- [ ] **Step 3: Latency guard** in `latency.rs`: the cost of 200 pixel deltas of 3 px on a 1k-line and a 100k-line document stays within the file's existing flatness bound. Inject an O(document) walk into the pixel path once; see it fail; restore.
- [ ] **Step 4:** Full tests, clippy, fmt; render at `--at-ms` mid-glide. **Commit** — "Scroll by pixels so the touchpad works, and glide the wheel".

### Task 7: Animate the chrome

**Files:** `crates/dacode/src/chrome.rs`; `crates/dacode/src/main.rs` (start animations where state changes); `crates/dacode/src/layout.rs` (`Layout::of(bounds, scale, results_rows, sidebar: f32)`, 0.0 closed to 1.0 open); `crates/dacode-render/src/frame.rs` (`Frame::icon_rotated`).

**Interfaces:**
- Consumes: Tasks 4–5 (`Key::{Sidebar, Palette, Menu, Completion, Info, Help, Notifications, TabIndicator, ActivityIndicator, Bell, Welcome, Hover(id)}`).
- Produces: `fn appear(rect: Rect, progress: f32, anchor: Anchor) -> Rect` in `chrome.rs` (scales `rect` about its anchor from 0.96 to 1.0; callers multiply every colour's alpha by `progress.clamp(0.0, 1.0)`); `enum Anchor { Top, Bottom, Centre }`; `Layout::of(.., sidebar: f32)`; `Frame::icon_rotated(icon, rect, color, radians)`.

- [ ] **Step 1: Failing tests:** `Layout::of` with `sidebar = 0.5` gives a sidebar half the full width and the editor column starts where it ends (the partition test, generalised over 0.0, 0.5, 1.0); a chrome test: the palette drawn at progress 0.0 adds no quad or run with alpha above 0.
- [ ] **Step 2:** Implement per the spec's table. Popups use `appear` with `Spring::POPUP` and fade; the sidebar width follows `Key::Sidebar` with `Spring::PANEL` (`state.sidebar_open()` sets the target; layout reads the current value); activity icons lift 2 px on hover (`Key::Hover(index)`, `Spring::HOVER`) and the active bar's y glides (`Key::ActivityIndicator`); the active tab's background x glides (`Key::TabIndicator`); the bell swings ±12° decaying (`Key::Bell`, kicked on `Chrome::notify`); welcome steps cross-fade (`Key::Welcome`). Targets are set in `main.rs` where state changes, never inside drawing.
- [ ] **Step 3:** Render the palette, a menu and the sidebar at `--at-ms 0`, `80`, `160`, `600`; read the PNGs.
- [ ] **Step 4:** Full tests (latency stays flat), clippy, fmt. **Commit** — "Animate popups, the sidebar, tabs, the activity bar and the bell".

### Task 8: An always-visible menu bar; no account icon or Share button

**Files:** `crates/dacode/src/layout.rs` (`title_bar`, `TitleBar`); `crates/dacode/src/menu.rs` (titles, items, `geometry`); `crates/dacode/src/chrome.rs` (`draw_title`, `draw_tooltip`); `crates/dacode/src/main.rs` (`click_chrome`, hover-switching in `CursorMoved`); `crates/dacode-input/src/commands.rs` (`share`'s description).

**Interfaces:**
- Produces: `TitleBar { logo: Rect, name: Rect, menus: Vec<Rect>, search: Option<Rect>, bell: Rect }` (no `account`, no `share`); `menu::TITLES: [&str; 8] = ["File", "Edit", "Selection", "View", "Go", "Run", "Terminal", "Help"]`; `layout::title_bar(title: Rect, scale: f32, typography: &Typography) -> TitleBar` (menus sized from `TITLES` with the `Ui` role's advance).

- [ ] **Step 1: Failing tests** (`layout.rs`): the menu rects sit inside the title, after the name, before the search box when there is one, never overlapping, at widths 360–2560 and scales 1–2; when they do not all fit, trailing titles drop. (`menu.rs`): every item names a command in `COMMANDS` (extend the existing test to the new menus); File contains `share`.
- [ ] **Step 2:** Implement. Menus — File: new_file, open_folder, save, save_as, share, close_buffer, settings, quit. Edit: undo, redo, cut, copy, paste, search, replace, search_project, toggle_comment. Selection: select_all, select_line, select_next_occurrence, select_all_occurrences, add_caret_above, add_caret_below. View: command_palette, toggle_tree, toggle_diagnostics, notifications, toggle_soft_wrap, split_vertical, split_horizontal, increase_font_size, decrease_font_size, toggle_theme. Go: goto_definition, find_references, goto_symbol, goto_symbol_in_project, jump_back, jump_forward, next_diagnostic, previous_diagnostic. Run: run. Terminal: run (labelled "copy the run command" until the terminal exists). Help: help, command_palette. A click on a title opens it; while one is open, pointer motion over another title opens that one. Remove `account` and `share` from drawing, hit-testing and tooltips.
- [ ] **Step 3:** Render `--menu` at 1600×1000 and 640×480; read them. **Commit** — "Put the menu bar in the title bar and drop the account and Share buttons".

### Task 9: One card style for every overlay

**Files:** `crates/dacode/src/chrome.rs` (`draw_palette`, `draw_menu`, `draw_completion`, `draw_info`, `draw_which_key`, `draw_help`, `draw_results`, `draw_diagnostics_list`, the `draw_welcome*` functions, `draw_empty_state`); `crates/dacode/src/palette.rs`, `crates/dacode/src/help.rs` (geometry); `crates/dacode/src/main.rs` (clicks on palette and help rows).

**Interfaces:**
- Consumes: `TitleBar.search` (Task 8) as the palette's anchor; `appear` (Task 7).
- Produces: `fn card(frame: &mut Frame, theme: &Theme, rect: Rect, scale: f32, alpha: f32)` (shadow: two offset translucent rounded fills; surface `bg_panel`, radius 8, 1 px `rule` outline); `fn key_chip(frame: &mut Frame, state: &EditorState, keys: &str, right: f32, y: f32, height: f32, scale: f32) -> f32` (rounded chips right-aligned at `right`, returns their left x); `palette::Geometry { card: Rect, input: Rect, rows: Vec<Rect> }` from `Palette::geometry(&self, bounds: Rect, anchor: Option<Rect>, scale: f32)`; `help::Geometry { card: Rect, rows: Vec<Rect> }` from `Help::geometry(&self, bounds: Rect, scale: f32)`.

- [ ] **Step 1: Failing tests:** palette geometry hangs from the command centre (top = anchor's bottom + 6 px, horizontally centred on it), rows stay inside the card, at sizes 640–2560; a wiring test clicks row *n*'s rectangle through `App::click` and the palette runs item *n*; the same for the help panel.
- [ ] **Step 2:** Implement `card` and `key_chip`; restyle each overlay. Keep headings that explain (palette groups "recent files" / "commands", help sections) and remove captions that do not. Keys use `Command::shown(modal)`.
- [ ] **Step 3:** Render each overlay at two sizes; read them. **Commit** — "Give every overlay the card style and make palette and help rows clickable".

### Task 10: The settings page — model and geometry

**Files:** `crates/dacode/src/settings_ui.rs`.

**Interfaces — produces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category { Appearance, Editor, Files, Keys, Languages, Import }
impl Category { pub const ALL: [Category; 6]; pub fn title(self) -> &'static str; }
pub struct Section { pub title: &'static str, pub fields: &'static [Field] }
pub fn sections(category: Category) -> &'static [Section];
pub enum Control { Switch, Stepper, Choice(&'static [&'static str]) }
impl Field {
    pub fn label(self, modal: bool) -> &'static str;
    pub fn explanation(self, modal: bool) -> Option<&'static str>;
    pub fn control(self) -> Control;
    pub fn is_default(self, values: &Values) -> bool;
}
pub struct Page { pub category: Category, pub query: String, pub scroll: usize, pub selected: usize }
pub struct PageGeometry { pub search: Rect, pub tree: Vec<(Category, Rect)>, pub rows: Vec<PageRow> }
pub struct PageRow { pub field: Option<Field>, pub heading: Option<&'static str>, pub rect: Rect, pub control: Option<Rect>, pub reset: Option<Rect> }
impl Page {
    pub fn geometry(&self, rect: Rect, scale: f32, values: &Values, modal: bool, typography: &Typography) -> PageGeometry;
    pub fn matches(&self, field: Field, modal: bool) -> bool;
}
```

Sections — Appearance: "Theme" (theme), "Text" (font_size), "Window" (transparency, animations). Editor: "Editing" (vim_keys, tab_width), "Wrapping" (soft_wrap), "Display" (line_numbers, sticky_scroll, inlay_hints). Files: "Saving" (autosave, format_on_save). Keys: the command list. Languages: the server and formatter report. Import: the VSCode checklist.

Explanations exist for vim_keys, line_numbers (one clause per choice), sticky_scroll, inlay_hints, autosave, format_on_save, transparency, animations, soft_wrap; `None` for font_size, tab_width and theme, whose names say everything.

- [ ] **Step 1: Failing tests:** every `Field` appears in exactly one section; a query "wrap" matches soft_wrap and nothing in other categories; geometry rows stay inside the page and do not overlap at 640×480 and 2560×1440; a field differing from its default has a `reset` rect and one at its default does not; no row's heading is a source caption (`"init.lua"`, `"imported"`, `"default"`).
- [ ] **Step 2:** Implement; leave the old `panel`/`Settings` in place until Task 11 removes them.
- [ ] **Step 3:** clippy, fmt. **Commit** — "Model the settings page: categories, sections, explanations and search".

### Task 11: The settings tab — drawing, input, auto-save

**Files:** `crates/dacode/src/chrome.rs` (`draw_settings_page`; delete `draw_settings`); `crates/dacode/src/main.rs` (the tab, input, auto-save, removal of the panel's handlers, `--settings`/`--row` capture); `crates/dacode/src/execute.rs` (`Settings` opens or focuses the tab); `crates/dacode/src/layout.rs` (the Settings activity opens the tab).

**Interfaces:**
- Consumes: Task 10; `Key::{Glow, SettingsCategory}` (Task 4).
- Produces: `App.settings_page: Option<settings_ui::Page>`; `App.settings_changed_at: Option<Instant>`; `const SETTINGS_SAVE_AFTER: Duration = Duration::from_millis(600)`; `App::save_settings_if_due(&mut self, now: Instant) -> bool`.

- [ ] **Step 1: Failing tests:** three adjustments in quick succession write `init.lua` once, 600 ms after the last, through `write_back` (call `save_settings_if_due` with explicit instants); the written file round-trips through `dacode_lua::eval`; a failed write (config directory read-only) is reported on the hover line and in the bell; the Settings activity icon opens the tab, and the tab bar lists "Settings" with the gear icon.
- [ ] **Step 2:** Implement the tab as an entry in the tab bar that is not a file: selecting it draws the page in the editor area instead of code; closing it works like closing a buffer. Keys with the tab focused: ↑/↓ rows, ←/→ adjust, space toggles, printable characters edit the search box, `esc` clears the search, then leaves the tab. Mouse: tree rows choose the category, controls adjust, ⟲ resets. A change glows (`Key::Glow(field index)`); categories cross-fade (`Key::SettingsCategory`). `about_to_wait` calls `save_settings_if_due(Instant::now())`.
- [ ] **Step 3:** Remove the settings sidebar panel and its handlers; `--settings` renders the tab, `--row N` selects row N on the page.
- [ ] **Step 4:** Render each category and a search at 1600×1000 and 640×480; read them. **Commit** — "Replace the settings panel with an auto-saving settings tab".

### Task 12: Documentation

**Files:** `CLAUDE.md` (Where things are: wake, focus, motion, scroll, settings page; Known gaps: palette and help rows are clickable now), `CHANGELOG.md` (Unreleased), `docs/reference/settings.md` (`animations`), `docs/reference/cli.md` (`--at-ms`), `docs/guides/configuration.md` (the settings tab and auto-save), `PLAN.md` (a paragraph per decision, with what was rejected).

- [ ] **Step 1:** Write the entries; `cargo test -p dacode --test conventions` (links resolve, install commands name the version).
- [ ] **Step 2: Commit** — "Document the overhaul".

---

## Self-review

- **Spec coverage:** §1 → Task 8; §2 → Tasks 4, 5, 7; §2a → Task 6; §3 → Task 9; §4 → Tasks 10, 11; §5 → Task 3; §6 → Tasks 1, 2; Testing → each task's Step 1; documentation → Task 12.
- **Names across tasks:** `Waker`/`Wake` (1 → 2); `App::new(.., waker)` (2 → every later test helper); `Focus` (3 → 11); `Motion`, `Key`, `Spring`, `Animations` (4 → 5, 6, 7, 11); `ScrollInput`, `CodeGrid::with_sub_row` (6); `Layout::of(.., sidebar: f32)` (7 → 8, 9, 11 — Task 3's test written with `true` becomes `1.0` in Task 7); `TitleBar.search` (8 → 9); `Page`, `PageGeometry`, `Category`, `Field::explanation` (10 → 11).
