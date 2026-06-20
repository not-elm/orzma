# ozbrowser keyboard link hints (Vimium-style) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Vimium-style `f` hint mode to ozbrowser so links, buttons, and form fields can be followed/operated with the keyboard alone.

**Architecture:** The ratatui TUI (`apps/ozbrowser`) owns a new `Mode::Hint` and forwards each hint keystroke to the webview as an emitted event; a host-injected preload script (`src/webview_render/ozma_hints.js`) overlays labels on the page, filters them, activates the chosen element, and reports the outcome back over the `window.ozma` channel so the TUI can switch modes. The page never takes keyboard focus (Hint mode is unfocused like Normal), so there is no focus-handoff race.

**Tech Stack:** Rust (ratatui, crossterm, serde_json, crossbeam-channel), plain browser JS (host preload, sibling to `ozma_bridge.js`), Bevy (`bevy_cef` `PreloadScripts`).

## Global Constraints

- Rust edition 2024, toolchain pinned to 1.95. All in-code comments in **English**.
- Rust comment taxonomy: only `// TODO:`, `// NOTE:` (critical caveat only), `// SAFETY:`. No `mod.rs`. All `use` at top of file, single contiguous block, no inline fully-qualified paths.
- Every externally-`pub` Rust item gets a `///` doc; default to the narrowest visibility that compiles (these crates already use `pub(crate)`/private heavily — match that).
- ratatui `Query` naming and Bevy `run_if`/change-detection rules apply to host-side Rust (`src/`).
- JS comments: only `// TODO:` / `// NOTE:` (critical caveat) / `// biome-ignore` / `// @ts-expect-error`. `ozma_hints.js` is plain JS injected via `include_str!` (mirrors `ozma_bridge.js`); it is **not** covered by vitest (the host crate is not a pnpm package).
- Preload ordering invariant: `ozma_hints.js` runs **after** `ozma_bridge.js` (it consumes `window.ozma`).
- Run Rust tests with `cargo test -p ozbrowser` (TUI) and `cargo test` (host crate, package `ozmux-gui`). Lint/format via `make fix-lint` before each commit if convenient; at minimum `cargo fmt`.

---

### Task 1: TUI keymap — `Mode::Hint` + hint actions + `f` trigger

**Files:**
- Modify: `apps/ozbrowser/src/keymap.rs`
- Test: `apps/ozbrowser/src/keymap.rs` (the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces (consumed by Task 2 & 3):
  - `Mode::Hint` variant on the existing `pub(crate) enum Mode`.
  - `Action::EnterHint`, `Action::HintKey(char)`, `Action::HintBackspace` on the existing `pub(crate) enum Action`.
  - `map(Mode::Normal, key 'f') == Action::EnterHint`.
  - In `Mode::Hint`: printable `Char(c)` → `Action::HintKey(c)`, `Backspace` → `Action::HintBackspace`, `Esc` → `Action::Escape`, `Ctrl-c` → `Action::Quit`, else `Action::Ignore`.

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `mod tests` block in `apps/ozbrowser/src/keymap.rs`:

```rust
    #[test]
    fn normal_f_enters_hint_mode() {
        assert_eq!(map(Mode::Normal, key('f')), Action::EnterHint);
    }

    #[test]
    fn hint_mode_printable_char_is_hint_key() {
        assert_eq!(map(Mode::Hint, key('a')), Action::HintKey('a'));
        assert_eq!(map(Mode::Hint, key('s')), Action::HintKey('s'));
    }

    #[test]
    fn hint_mode_backspace_and_escape() {
        assert_eq!(
            map(Mode::Hint, special(KeyCode::Backspace)),
            Action::HintBackspace
        );
        assert_eq!(map(Mode::Hint, special(KeyCode::Esc)), Action::Escape);
    }

    #[test]
    fn hint_mode_ctrl_c_quits_and_other_ctrl_ignored() {
        assert_eq!(map(Mode::Hint, ctrl('c')), Action::Quit);
        assert_eq!(map(Mode::Hint, ctrl('d')), Action::Ignore);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ozbrowser keymap`
Expected: FAIL to compile — `no variant named Hint`, `EnterHint`, `HintKey`, `HintBackspace`.

- [ ] **Step 3: Add the enum variants**

In `apps/ozbrowser/src/keymap.rs`, extend `Mode`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Normal,
    Insert,
    Address,
    Help,
    Hint,
}
```

Extend `Action` (add the three new variants alongside the others):

```rust
    ScrollPageUp,
    GoBottom,
    Prefix(char),
    HistoryBack,
    HistoryForward,
    OpenAddress,
    Reload,
    EnterInsert,
    EnterHint,
    OpenHelp,
    AddressChar(char),
    AddressBackspace,
    AddressConfirm,
    HintKey(char),
    HintBackspace,
    Escape,
    Quit,
    Ignore,
```

- [ ] **Step 4: Bind `f` in Normal and add the Hint arm**

In `map`, add a `Mode::Hint` arm:

```rust
    match mode {
        Mode::Normal => map_normal(ctrl, key.code),
        Mode::Hint => map_hint(ctrl, key.code),
        Mode::Insert => match key.code {
```

In `map_normal`, add the `f` binding among the plain-char arms (after the `_ if ctrl => Action::Ignore` guard, so `Ctrl-f` keeps mapping to `ScrollPageDown`):

```rust
        KeyCode::Char('i') => Action::EnterInsert,
        KeyCode::Char('f') => Action::EnterHint,
        KeyCode::Char('?') => Action::OpenHelp,
```

Add the `map_hint` helper next to `map_normal` (private; both callers are in this file):

```rust
fn map_hint(ctrl: bool, code: KeyCode) -> Action {
    match code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        _ if ctrl => Action::Ignore,
        KeyCode::Esc => Action::Escape,
        KeyCode::Backspace => Action::HintBackspace,
        KeyCode::Char(c) => Action::HintKey(c),
        _ => Action::Ignore,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ozbrowser keymap`
Expected: PASS (new tests + all existing keymap tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add apps/ozbrowser/src/keymap.rs
git commit -m "feat(ozbrowser): keymap Mode::Hint, f trigger, hint-key mapping"
```

---

### Task 2: TUI app state — hint commands + transitions

**Files:**
- Modify: `apps/ozbrowser/src/app.rs`
- Test: `apps/ozbrowser/src/app.rs` (the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (from Task 1): `Mode::Hint`, `Action::EnterHint`, `Action::HintKey(char)`, `Action::HintBackspace`.
- Produces (consumed by Task 3):
  - `Cmd::HintShow`, `Cmd::HintKey(char)`, `Cmd::HintBackspace`, `Cmd::HintHide` on the existing `pub(crate) enum Cmd`.
  - `App::on_action(Action::EnterHint)` sets `Mode::Hint`, returns `vec![Cmd::HintShow]`.
  - `App::on_action(Action::HintKey(c))` returns `vec![Cmd::HintKey(c)]` (mode unchanged).
  - `App::on_action(Action::HintBackspace)` returns `vec![Cmd::HintBackspace]`.
  - `App::on_action(Action::Escape)` while in `Mode::Hint` returns `vec![Cmd::HintHide]` and sets `Mode::Normal`.
  - `pub(crate) fn App::on_hint_result(&mut self, kind: &str)` — `"focusedInput"` → `Mode::Insert`, anything else → `Mode::Normal`; a no-op unless currently in `Mode::Hint`.

- [ ] **Step 1: Write the failing tests**

Add inside the existing `mod tests` block in `apps/ozbrowser/src/app.rs`:

```rust
    #[test]
    fn enter_hint_sets_hint_mode_and_emits_show() {
        let mut a = app();
        assert_eq!(a.on_action(Action::EnterHint), vec![Cmd::HintShow]);
        assert_eq!(a.mode(), Mode::Hint);
    }

    #[test]
    fn hint_key_and_backspace_emit_commands_without_mode_change() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_action(Action::HintKey('a')), vec![Cmd::HintKey('a')]);
        assert_eq!(a.mode(), Mode::Hint);
        assert_eq!(a.on_action(Action::HintBackspace), vec![Cmd::HintBackspace]);
        assert_eq!(a.mode(), Mode::Hint);
    }

    #[test]
    fn escape_from_hint_mode_hides_and_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::HintHide]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    #[test]
    fn hint_result_focused_input_switches_to_insert() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        a.on_hint_result("focusedInput");
        assert_eq!(a.mode(), Mode::Insert);
    }

    #[test]
    fn hint_result_navigated_or_clicked_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        a.on_hint_result("navigated");
        assert_eq!(a.mode(), Mode::Normal);
    }

    #[test]
    fn hint_result_is_ignored_when_not_in_hint_mode() {
        let mut a = app();
        a.on_hint_result("focusedInput");
        assert_eq!(a.mode(), Mode::Normal);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ozbrowser app`
Expected: FAIL to compile — `no variant named HintShow`/`HintKey`/`HintBackspace`/`HintHide`, `no method on_hint_result`.

- [ ] **Step 3: Add the `Cmd` variants**

In `apps/ozbrowser/src/app.rs`, extend `Cmd`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Navigate to the given URL.
    Navigate(String),
    /// Navigate back in history.
    HistoryBack,
    /// Navigate forward in history.
    HistoryForward,
    /// Reload the current page.
    Reload,
    /// Scroll the webview.
    Scroll(ScrollAction),
    /// Show the link-hint overlay on the page.
    HintShow,
    /// Forward a typed hint-label character to the page.
    HintKey(char),
    /// Forward a hint-label backspace to the page.
    HintBackspace,
    /// Tear down the link-hint overlay on the page.
    HintHide,
    /// Exit the app.
    Quit,
}
```

- [ ] **Step 4: Handle the new actions in `on_action` and add `on_hint_result`**

In `on_action`, add arms (place `EnterHint` near `EnterInsert`, the hint-key arms near the address arms) and change the `Escape` arm to emit `HintHide` when leaving Hint mode:

```rust
            Action::EnterInsert => {
                self.mode = Mode::Insert;
                vec![]
            }
            Action::EnterHint => {
                self.mode = Mode::Hint;
                vec![Cmd::HintShow]
            }
            Action::HintKey(c) => vec![Cmd::HintKey(c)],
            Action::HintBackspace => vec![Cmd::HintBackspace],
```

Replace the existing `Action::Escape` arm with:

```rust
            Action::Escape => {
                let was_hint = self.mode == Mode::Hint;
                self.mode = Mode::Normal;
                self.address_buf.clear();
                if was_hint {
                    vec![Cmd::HintHide]
                } else {
                    vec![]
                }
            }
```

Add the `on_hint_result` method to the `impl App` block (after `set_url`, with the other `pub(crate)` methods, before the private `resolve_chord`):

```rust
    /// Applies a `hintResult` reported by the page: a hint that focused a form
    /// field switches to Insert mode; any other resolution returns to Normal.
    /// A no-op unless currently in Hint mode (guards against a late result
    /// arriving after the user already cancelled with Esc).
    pub(crate) fn on_hint_result(&mut self, kind: &str) {
        if self.mode != Mode::Hint {
            return;
        }
        self.mode = if kind == "focusedInput" {
            Mode::Insert
        } else {
            Mode::Normal
        };
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ozbrowser app`
Expected: PASS (new tests + all existing app tests, including `escape_from_address_mode_returns_to_normal` and `escape_from_insert_returns_to_normal`, which still hit the `was_hint == false` branch).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add apps/ozbrowser/src/app.rs
git commit -m "feat(ozbrowser): hint commands + on_hint_result mode transitions"
```

---

### Task 3: TUI event loop + chrome wiring

**Files:**
- Modify: `apps/ozbrowser/src/main.rs`
- Modify: `apps/ozbrowser/src/ui.rs`

**Interfaces:**
- Consumes (from Task 1 & 2): `Mode::Hint`, all four `Cmd::Hint*` variants, `App::on_hint_result`.
- Produces (consumed by Task 4's JS at runtime): emits the events `hints:show`, `hints:key` (`{ key }` or `{ backspace: true }`), `hints:hide`, and registers an `on("hintResult", …)` RPC handler that reads `args["kind"]`.

This task is integration glue (no unit tests; ozbrowser has none for `main.rs`/`ui.rs`). Verify by `cargo build` + `cargo test -p ozbrowser` (the existing suites must still pass) and a manual run.

- [ ] **Step 1: Add a hint-result channel and thread it through `register_view`**

In `apps/ozbrowser/src/main.rs`, in `run()`, create the channel right after the existing `url_tx`/`url_rx` pair:

```rust
    let (url_tx, url_rx) = crossbeam_channel::unbounded::<String>();
    let (hint_tx, hint_rx) = crossbeam_channel::unbounded::<String>();
    let view = register_view(&ozma, &initial_url, url_tx.clone(), hint_tx.clone())?;
```

Update the `event_loop` call to pass the hint channel:

```rust
    let result = event_loop(view, App::new(initial_url), &ozma, url_tx, &url_rx, hint_tx, &hint_rx);
```

- [ ] **Step 2: Update `event_loop` signature, drain the channel, and emit the hint commands**

Change `event_loop`'s signature to accept the hint channel ends (mutable params first per the repo rule — both `Sender`s are owned values the function moves into closures, the `Receiver`s are borrows; keep the existing ordering style by grouping the new params with their url counterparts):

```rust
fn event_loop(
    mut view: WebviewHandle,
    mut app: App,
    ozma: &Ozma,
    url_tx: Sender<String>,
    url_rx: &Receiver<String>,
    hint_tx: Sender<String>,
    hint_rx: &Receiver<String>,
) -> anyhow::Result<()> {
```

At the top of the loop, drain `hint_rx` next to the existing `url_rx` drain:

```rust
        while let Ok(url) = url_rx.try_recv() {
            app.set_url(url);
        }
        while let Ok(kind) = hint_rx.try_recv() {
            app.on_hint_result(&kind);
        }
```

In the `match cmd` block, add the four hint arms and pass `hint_tx.clone()` to every `register_view` call. The `Navigate` / `HistoryBack` / `HistoryForward` / `Reload` arms each call `register_view(...)` — update them to the new four-arg form:

```rust
                    Cmd::Navigate(url) => {
                        let url = app.navigate(url);
                        view = register_view(ozma, &url, url_tx.clone(), hint_tx.clone())?;
                    }
                    Cmd::HistoryBack => {
                        if let Some(url) = app.go_back() {
                            view = register_view(ozma, &url, url_tx.clone(), hint_tx.clone())?;
                        }
                    }
                    Cmd::HistoryForward => {
                        if let Some(url) = app.go_forward() {
                            view = register_view(ozma, &url, url_tx.clone(), hint_tx.clone())?;
                        }
                    }
                    Cmd::Reload => {
                        view = register_view(ozma, app.url(), url_tx.clone(), hint_tx.clone())?;
                    }
                    Cmd::Scroll(action) => {
                        let _ = view.emit("scroll", &scroll_payload(action));
                    }
                    Cmd::HintShow => {
                        let _ = view.emit("hints:show", &serde_json::json!({}));
                    }
                    Cmd::HintKey(c) => {
                        let _ = view.emit("hints:key", &serde_json::json!({ "key": c.to_string() }));
                    }
                    Cmd::HintBackspace => {
                        let _ = view.emit("hints:key", &serde_json::json!({ "backspace": true }));
                    }
                    Cmd::HintHide => {
                        let _ = view.emit("hints:hide", &serde_json::json!({}));
                    }
```

- [ ] **Step 3: Register the `hintResult` handler in `register_view`**

Change `register_view`'s signature and add the handler. The function currently ends its builder chain with `.on("urlChanged", …)`; add a second `.on("hintResult", …)` to the same chain:

```rust
fn register_view(
    ozma: &Ozma,
    url: &str,
    url_tx: Sender<String>,
    hint_tx: Sender<String>,
) -> anyhow::Result<WebviewHandle> {
```

Replace the final `let view = ozma.register(...)?;` builder with the two-handler chain (keep the existing `pass` array and `urlChanged` body verbatim; only add the `hintResult` handler):

```rust
    let view = ozma.register(
        Webview::url(url)
            .interactive(true)
            .passthrough(pass)
            .on(
                "urlChanged",
                move |args: serde_json::Value| -> Result<(), RpcError> {
                    if let Some(u) = args["url"].as_str() {
                        let _ = url_tx.send(u.to_owned());
                    }
                    Ok(())
                },
            )
            .on(
                "hintResult",
                move |args: serde_json::Value| -> Result<(), RpcError> {
                    if let Some(kind) = args["kind"].as_str() {
                        let _ = hint_tx.send(kind.to_owned());
                    }
                    Ok(())
                },
            ),
    )?;
    Ok(view)
```

- [ ] **Step 4: Add the Hint status label and help line in `ui.rs`**

In `apps/ozbrowser/src/ui.rs`, add the `Hint` arm to the `mode_label` match in `draw_status_bar`:

```rust
    let mode_label = match app.mode() {
        Mode::Normal => "Normal",
        Mode::Insert => "Insert",
        Mode::Help => "Help",
        Mode::Address => "Address",
        Mode::Hint => "Hint",
    };
```

Add a help line in `draw_help_modal`'s `lines` vec (right after the `i` line):

```rust
        Line::from("  i              insert mode (focus webview)"),
        Line::from("  f              follow link / hint"),
        Line::from("  ?              this help"),
```

- [ ] **Step 5: Build and run the existing suites**

Run: `cargo build -p ozbrowser && cargo test -p ozbrowser`
Expected: builds clean; all existing + Task 1/2 tests PASS. (No new unit tests in this task — it is wiring.)

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add apps/ozbrowser/src/main.rs apps/ozbrowser/src/ui.rs
git commit -m "feat(ozbrowser): wire hint events, hintResult channel, Hint chrome"
```

---

### Task 4: Host hint engine + URL-webview preload injection

**Files:**
- Create: `src/webview_render/ozma_hints.js`
- Modify: `src/webview_render/preload.rs`
- Modify: `src/inline_webview.rs`
- Test: `src/webview_render/preload.rs` (the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (at runtime, from Task 3): the events `hints:show`, `hints:key`, `hints:hide`; calls back `hintResult` with `{ kind: "navigated" | "clicked" | "focusedInput" | "empty" }`.
- Produces:
  - `pub(super) const OZMA_HINTS_JS: &str` in `preload.rs`.
  - `pub(crate) fn build_url_preload() -> PreloadScripts` returning `[OZMA_BRIDGE_JS, OZMA_HINTS_JS]` in that order.
  - A new `pub(crate) is_url: bool` field on `ResolvedWebviewMount`, set true for a `DynSource::Url` source.
  - `mount_inline` inserts `build_url_preload()` for a bridged URL view and `build_dynamic_preload()` for a bridged inline/dir view.

- [ ] **Step 1: Write the failing Rust tests**

In `src/webview_render/preload.rs`, add tests to the existing `mod tests`:

```rust
    #[test]
    fn url_preload_injects_bridge_then_hints_in_order() {
        let preload = build_url_preload();
        assert_eq!(preload.0.len(), 2, "bridge + hints");
        assert_eq!(preload.0[0], OZMA_BRIDGE_JS, "bridge must run first");
        assert_eq!(preload.0[1], OZMA_HINTS_JS, "hints run after the bridge");
        assert!(OZMA_HINTS_JS.contains("hints:show"));
        assert!(OZMA_HINTS_JS.contains("hintResult"));
    }

    #[test]
    fn dynamic_preload_has_no_hints() {
        let preload = build_dynamic_preload();
        assert!(
            !preload.0.iter().any(|s| s.contains("hints:show")),
            "inline/dir webviews must not carry the hint engine"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ozmux-gui preload`
Expected: FAIL to compile — `cannot find function build_url_preload`, `cannot find value OZMA_HINTS_JS`.

- [ ] **Step 3: Create `ozma_hints.js`**

Create `src/webview_render/ozma_hints.js` with the full hint engine:

```javascript
// NOTE: preload ordering — this script runs AFTER ozma_bridge.js, so window.ozma
// is already defined and frozen; injected before the bridge it would throw on the
// `window.ozma` reads below.
(function () {
  var ozma = window.ozma;
  var ALPHABET = 'sadfjklewcmpgh';
  var OVERLAY_ID = '__ozmaHints';
  var state = null;

  function isVisible(el) {
    var r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return false;
    if (r.bottom < 0 || r.right < 0 || r.top > window.innerHeight || r.left > window.innerWidth) {
      return false;
    }
    var s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
  }

  function classify(el) {
    var tag = el.tagName.toLowerCase();
    if (tag === 'a' && el.hasAttribute('href')) return 'link';
    if (tag === 'textarea' || tag === 'select') return 'input';
    if (tag === 'input') {
      var t = (el.getAttribute('type') || 'text').toLowerCase();
      var clickable = t === 'button' || t === 'submit' || t === 'reset' ||
        t === 'checkbox' || t === 'radio' || t === 'file' || t === 'image';
      return clickable ? 'button' : 'input';
    }
    return 'button';
  }

  function generateLabels(n) {
    var a = ALPHABET, k = a.length, labels = [];
    if (n <= k) {
      for (var i = 0; i < n; i++) labels.push(a[i]);
      return labels;
    }
    for (var i = 0; i < k && labels.length < n; i++) {
      for (var j = 0; j < k && labels.length < n; j++) {
        labels.push(a[i] + a[j]);
      }
    }
    return labels;
  }

  function teardown() {
    var o = document.getElementById(OVERLAY_ID);
    if (o) o.remove();
    state = null;
  }

  function show() {
    teardown();
    var sel = 'a[href], button, input, textarea, select, [role=button], [onclick]';
    var els = Array.prototype.slice.call(document.querySelectorAll(sel)).filter(isVisible);
    if (els.length === 0) {
      ozma.call('hintResult', { kind: 'empty' });
      return;
    }
    var labels = generateLabels(els.length);
    var overlay = document.createElement('div');
    overlay.id = OVERLAY_ID;
    overlay.setAttribute('style', 'position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;');
    var targets = [];
    for (var i = 0; i < els.length; i++) {
      var el = els[i];
      var label = labels[i];
      var r = el.getBoundingClientRect();
      var badge = document.createElement('div');
      badge.textContent = label.toUpperCase();
      badge.setAttribute('style',
        'position:fixed;left:' + Math.max(0, Math.floor(r.left)) + 'px;' +
        'top:' + Math.max(0, Math.floor(r.top)) + 'px;' +
        'background:#ffd76e;color:#302505;font:bold 11px/14px monospace;' +
        'padding:0 3px;border-radius:3px;box-shadow:0 1px 2px rgba(0,0,0,.4);');
      overlay.appendChild(badge);
      targets.push({ el: el, label: label, kind: classify(el), badge: badge });
    }
    document.documentElement.appendChild(overlay);
    state = { targets: targets, prefix: '' };
  }

  function activate(t) {
    teardown();
    if (t.kind === 'input') {
      t.el.focus();
      ozma.call('hintResult', { kind: 'focusedInput' });
    } else {
      t.el.click();
      ozma.call('hintResult', { kind: t.kind === 'link' ? 'navigated' : 'clicked' });
    }
  }

  function refilter() {
    var p = state.prefix;
    var match = null;
    var remaining = 0;
    for (var i = 0; i < state.targets.length; i++) {
      var t = state.targets[i];
      var hit = t.label.indexOf(p) === 0;
      t.badge.style.display = hit ? '' : 'none';
      if (hit) {
        remaining++;
        if (t.label === p) match = t;
      }
    }
    if (match && remaining === 1) activate(match);
  }

  ozma.on('hints:show', function () { show(); });
  ozma.on('hints:hide', function () { teardown(); });
  ozma.on('hints:key', function (payload) {
    if (!state) return;
    if (payload && payload.backspace) {
      state.prefix = state.prefix.slice(0, -1);
    } else {
      var key = payload && payload.key;
      if (!key) return;
      var ch = key.toLowerCase();
      if (ALPHABET.indexOf(ch) === -1) return;
      var next = state.prefix + ch;
      var any = state.targets.some(function (t) { return t.label.indexOf(next) === 0; });
      if (!any) return;
      state.prefix = next;
    }
    refilter();
  });
})();
```

- [ ] **Step 4: Add the preload constant and builder**

In `src/webview_render/preload.rs`, add the constant next to `OZMA_BRIDGE_JS` and a `build_url_preload` next to `build_dynamic_preload`:

```rust
/// JS defining the unified `window.ozma` back-channel bridge (`.call` / `.on`),
/// injected per Tier 1 dynamic webview as a `PreloadScripts` entry. Frozen onto
/// `window` so a page cannot shadow it.
pub(super) const OZMA_BRIDGE_JS: &str = include_str!("ozma_bridge.js");

/// JS implementing the Vimium-style link-hint engine (`hints:show` / `hints:key`
/// / `hints:hide` handlers, reporting `hintResult`). Injected after the bridge
/// for URL webviews, which it depends on for `window.ozma`.
const OZMA_HINTS_JS: &str = include_str!("ozma_hints.js");
```

> NOTE: `OZMA_HINTS_JS` is **private** (no modifier) — its only callers
> (`build_url_preload` and the tests) live in this module, and the
> MANDATORY visibility rule requires module-scoped items to be private.
> The `#[cfg(test)] mod tests` block is a descendant module, so `use
> super::*;` still brings the private constant into the test scope.
```

```rust
/// Builds the preload for a Tier 1 dynamic webview: the `window.ozma`
/// back-channel bridge. No capability grant — the bridge routes to the
/// registering program, not the host.
pub(crate) fn build_dynamic_preload() -> PreloadScripts {
    PreloadScripts::from([OZMA_BRIDGE_JS.to_string()])
}

/// Builds the preload for a bridged URL webview: the `window.ozma` bridge
/// followed by the link-hint engine. Order matters — the hint engine consumes
/// `window.ozma`, which the bridge defines, so the bridge entry is first.
pub(crate) fn build_url_preload() -> PreloadScripts {
    PreloadScripts::from([OZMA_BRIDGE_JS.to_string(), OZMA_HINTS_JS.to_string()])
}
```

- [ ] **Step 5: Run the preload tests to verify they pass**

Run: `cargo test -p ozmux-gui preload`
Expected: PASS (new tests + the existing `dynamic_preload_injects_only_the_ozma_bridge`).

- [ ] **Step 6: Carry URL-ness through the mount and branch the preload**

In `src/inline_webview.rs`, add the import (extend the existing `use crate::webview_render::preload::…` line — keep it a single contiguous import block):

```rust
use crate::webview_render::preload::{build_dynamic_preload, build_url_preload};
```

Add the field to `ResolvedWebviewMount` (after `owner`):

```rust
    pub(crate) owner: Option<(u64, String)>,
    /// Whether the resolved source is a remote `Url` (vs `Dir`/`Inline`). A
    /// bridged URL view additionally receives the link-hint preload.
    pub(crate) is_url: bool,
    /// The normalized passthrough chords copied from the registration, stamped
```

In `resolve_mount`, compute and set it. The function already binds `view` and matches `view.source`; add the flag and include it in the returned struct:

```rust
    let url = match &view.source {
        DynSource::Dir(_) => format!("ozma-dyn://{id}/{}", view.entry),
        DynSource::Inline(_) => format!("ozma-dyn://{id}/index.html"),
        DynSource::Url { url, .. } => url.clone(),
    };
    let is_url = matches!(view.source, DynSource::Url { .. });
    let owner = view
        .source
        .is_bridged()
        .then(|| (view.connection_id, id.to_string()));
    Some(ResolvedWebviewMount {
        url: Some(url),
        interactive: view.interactive,
        owner,
        is_url,
        passthrough: view.passthrough.clone(),
    })
```

In `mount_inline`, branch the preload at the `build_dynamic_preload()` insertion site (the `if let Some((connection_id, handle)) = resolved.owner` block):

```rust
    if let Some((connection_id, handle)) = resolved.owner {
        let preload = if resolved.is_url {
            build_url_preload()
        } else {
            build_dynamic_preload()
        };
        params.commands.entity(webview).insert((
            preload,
            WebviewOwner {
                connection_id,
                handle,
            },
        ));
    }
```

- [ ] **Step 7: Build and confirm no other literal sites were missed**

The only `ResolvedWebviewMount { … }` literal in the tree is the one in
`resolve_mount` (updated in Step 6); there are no test literals constructing it
directly. Confirm and build:

Run: `grep -rn "ResolvedWebviewMount {" src/`
Expected: two hits — the `struct` definition (line ~147) and the `Some(ResolvedWebviewMount {` literal in `resolve_mount` (already carrying `is_url`). If a future literal appears, add `is_url: <bool>` (`true` only where the case models a `DynSource::Url` mount).

Run: `cargo build -p ozmux-gui`
Expected: builds clean.

- [ ] **Step 8: Run the host crate tests**

Run: `cargo test -p ozmux-gui inline_webview preload`
Expected: PASS — existing inline_webview tests (now carrying `is_url`) and the new preload tests.

- [ ] **Step 9: Full workspace check + commit**

Run: `cargo test` and `cargo fmt`
Expected: workspace tests PASS.

```bash
git add -f src/webview_render/ozma_hints.js
git add src/webview_render/preload.rs src/inline_webview.rs
git commit -m "feat(webview): inject Vimium-style link-hint engine into URL webviews"
```

> NOTE: `src/` is **not** under the `docs/` gitignore rule, so `ozma_hints.js` adds normally; the `-f` above is belt-and-suspenders. Confirm with `git status` that the file is staged before committing.

---

## Manual verification (after Task 4)

Run ozbrowser inside an ozmux pane against a link-rich page and confirm:

1. `f` overlays yellow labels on links/buttons/fields; status bar shows `[Hint]`.
2. Typing a label's letters narrows then activates: a link navigates (URL bar updates), a button clicks in place (back to Normal), a text field focuses and the status bar flips to `[Insert]`.
3. `Esc` during hints clears the overlay and returns to `[Normal]`.
4. On a page with no visible targets, `f` flashes back to Normal without hanging.
5. `?` help lists the `f  follow link / hint` line.

## Self-Review notes (coverage map)

- Spec §1 targets/activation table → Task 4 `classify` + `activate`.
- Spec §3 flow + §4 race-free routing → Task 1 (`Mode::Hint` unfocused), Task 2 (transitions), Task 3 (emits + channel).
- Spec §5 page engine + label algorithm → Task 4 `ozma_hints.js` (`generateLabels` uniform-length prefix-free, `refilter`).
- Spec §5 injection wiring → Task 4 `build_url_preload` + `is_url` branch.
- Spec §6 edge cases → Task 4 (`empty`, non-alphabet ignore, zero-match ignore, stale-key no-op), Task 2 (`Esc` local, late-result guard).
- Spec §7 testing → Task 1/2 Rust unit tests, Task 4 preload-builder tests, manual verification list above.
- Spec §8 out-of-scope (history) → intentionally not implemented; link activation rides the existing `urlChanged` path (Task 3 leaves `register_view`'s `urlChanged` handler untouched).
