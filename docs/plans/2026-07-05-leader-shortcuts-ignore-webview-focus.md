# Leader shortcuts fire regardless of webview keyboard focus — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `<Leader>`-scoped shortcuts fire while a CEF webview owns the keyboard, and withhold the leader-claimed keys from the web page via bevy_cef's `CefKeyboardFilter`.

**Architecture:** The pure decider `classify_key_batch` (`src/input/resolve.rs`) gains a `ctx.webview_focused` path that steps the leader state machine instead of resetting it, returning a new `ClassifiedKeys { effects, webview_suppressed }`. The single ECS system `resolve_shortcuts` (`src/input/dispatch.rs`) forwards `webview_suppressed` to `CefKeyboardFilter`, ordered `.before(KeyboardDeliverSet)` so bevy_cef's delivery skips those keys the same frame.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18 ECS, bevy_cef 0.11 (crates.io) `CefKeyboardFilter` / `KeyboardDeliverSet` / `ModifiersState`.

## Global Constraints

- Rust edition 2024, toolchain pinned to 1.95.
- No `mod.rs`; comments only `// TODO:` / `// NOTE:` / `// SAFETY:`; all `use` at top of file, one contiguous block, no inline fully-qualified paths.
- Externally-`pub` items need `///` docs; `pub(crate)`/private do not (but keep names meaningful).
- Visibility minimization: default private; widen only for a real cross-module caller.
- Bevy: gate whole-system change checks with `run_if`; let mutation drive change detection (no manual `set_changed`); `Plugin::build` as one method chain; register systems in the defining file's plugin.
- Parameter ordering: mutable params before immutable (exception: a fixed leading `self` / `On<E>` slot).
- All in-code comments in English.
- Lint/format gate before every commit: `cargo clippy --workspace --all-targets` clean and `cargo fmt`.

---

## File Structure

- `src/input/resolve.rs` (modify) — pure decider. Add `ClassifiedKeys`; change `classify_key_batch`'s return type; rewrite the `ctx.webview_focused` branch to run the leader machine and record claimed keys. Owns all pure-layer unit tests.
- `src/input/dispatch.rs` (modify) — the `resolve_shortcuts` ECS system. Consume `ClassifiedKeys`, populate/clear `CefKeyboardFilter`, order `.before(KeyboardDeliverSet)`. Owns the ECS-layer tests.

No other files change. The appliers (`src/input/default_mode.rs`, `src/input/tmux/input.rs`) already handle `KeyEffect::Action { via_leader: true }` and are not gated on webview focus; `src/input/shortcuts.rs` (`step_leader`, `LeaderPhase`, `detect_modifier_tap`, `reset_leader_phase`) and `src/input/focus.rs` are unchanged.

---

## Task 1: Leader fires during webview focus + CEF key-leak suppression

**Files:**
- Modify: `src/input/resolve.rs` (the `KeyEffect`/`BatchContext` region ~15-65, `classify_key_batch` ~75-143, the `#[cfg(test)]` `run` helper ~230-238 and webview tests ~760-805)
- Modify: `src/input/dispatch.rs` (imports ~26-28, `resolve_shortcuts` ~110-185, `resolve_app` test helper ~222-250)

**Interfaces:**
- Produces (consumed by later tasks and by `resolve_shortcuts`):
  - `pub(crate) struct ClassifiedKeys { pub(crate) effects: Vec<KeyEffect>, pub(crate) webview_suppressed: Vec<KeyCode> }`
  - `fn classify_key_batch(leader_phase: &mut LeaderPhase, shortcuts: &Shortcuts, resolved_copy: &ResolvedCopyModeKeys, events: impl Iterator<Item = &KeyboardInput>, ctx: BatchContext) -> ClassifiedKeys` (return type changed from `Vec<KeyEffect>`)
- Consumes: bevy_cef `CefKeyboardFilter` (`fn set(impl IntoIterator<Item=(Entity, KeyCode, ModifiersState)>)`, `fn contains(Entity, KeyCode, ModifiersState) -> bool`) and `ModifiersState { alt, ctrl, shift, logo }` — both from `bevy_cef::prelude`.

---

- [ ] **Step 1: Add `ClassifiedKeys`, change the return type, and rewrite the webview branch in `src/input/resolve.rs`**

Add the struct just above `classify_key_batch` (after the `BatchContext` struct, before the `classify_key_batch` doc comment):

```rust
/// The decided output of `classify_key_batch`: the per-key `KeyEffect`s, plus the
/// physical keys the leader claimed while a webview owned the keyboard. The caller
/// applies the frame's modifier snapshot when withholding `webview_suppressed`
/// from CEF via `CefKeyboardFilter`; it is empty on the non-webview path.
pub(crate) struct ClassifiedKeys {
    pub(crate) effects: Vec<KeyEffect>,
    pub(crate) webview_suppressed: Vec<KeyCode>,
}
```

Change the `classify_key_batch` signature return type from `-> Vec<KeyEffect>` to `-> ClassifiedKeys`.

Immediately after `let mut effects = Vec::new();` add:

```rust
    let mut webview_suppressed = Vec::new();
```

Replace the existing `ctx.webview_focused` block (currently):

```rust
        if ctx.webview_focused {
            // NOTE: the leader is honored only when a webview does not own
            // the keyboard; resetting the phase here (instead of stepping
            // the state machine) prevents a stale leader from firing once
            // keyboard focus returns to the terminal.
            *leader_phase = LeaderPhase::Idle;
            if shortcuts.is_release_webview_focus(ev.key_code, ctx.mods) {
                effects.push(KeyEffect::ReleaseWebviewFocus);
            } else if ctx
                .forward_chords
                .iter()
                .any(|chord| chord_matches(chord, ev.key_code, ctx.mods))
            {
                effects.push(KeyEffect::WebviewForward {
                    logical: ev.logical_key.clone(),
                    key_code: ev.key_code,
                });
            }
            continue;
        }
```

with:

```rust
        if ctx.webview_focused {
            // NOTE: the leader runs even while a webview owns the keyboard, so
            // `<Leader>` shortcuts work regardless of focus. Keys the leader
            // claims (the leader chord itself, an abandoned second key, or a
            // fired binding) are recorded in `webview_suppressed` so the caller
            // withholds them from CEF; a key the leader does not claim
            // (`Passthrough`) still resolves to release / forward as before.
            match step_with_repeat(leader_phase, shortcuts, ev, ctx.mods, ctx.now) {
                LeaderStep::Swallow => {
                    webview_suppressed.push(ev.key_code);
                }
                LeaderStep::RunAction(action) => {
                    webview_suppressed.push(ev.key_code);
                    effects.push(KeyEffect::Action {
                        action,
                        via_leader: true,
                    });
                }
                LeaderStep::Passthrough => {
                    if shortcuts.is_release_webview_focus(ev.key_code, ctx.mods) {
                        effects.push(KeyEffect::ReleaseWebviewFocus);
                    } else if ctx
                        .forward_chords
                        .iter()
                        .any(|chord| chord_matches(chord, ev.key_code, ctx.mods))
                    {
                        effects.push(KeyEffect::WebviewForward {
                            logical: ev.logical_key.clone(),
                            key_code: ev.key_code,
                        });
                    }
                }
            }
            continue;
        }
```

Change the function's final expression from `effects` to:

```rust
    ClassifiedKeys {
        effects,
        webview_suppressed,
    }
```

(`step_with_repeat`, `LeaderStep`, and `chord_matches` are already in scope in this file — no new `use`.)

- [ ] **Step 2: Update the `run` test helper and add `run_full` in `src/input/resolve.rs`**

In the `#[cfg(test)] mod tests`, change the `run` helper's body to unwrap `.effects` so every existing effects-only test keeps compiling, and add a full-struct helper below it:

```rust
    fn run<'a>(
        leader_phase: &mut LeaderPhase,
        shortcuts: &Shortcuts,
        resolved_copy: &ResolvedCopyModeKeys,
        events: &'a [KeyboardInput],
        ctx: BatchContext<'a>,
    ) -> Vec<KeyEffect> {
        classify_key_batch(leader_phase, shortcuts, resolved_copy, events.iter(), ctx).effects
    }

    fn run_full<'a>(
        leader_phase: &mut LeaderPhase,
        shortcuts: &Shortcuts,
        resolved_copy: &ResolvedCopyModeKeys,
        events: &'a [KeyboardInput],
        ctx: BatchContext<'a>,
    ) -> ClassifiedKeys {
        classify_key_batch(leader_phase, shortcuts, resolved_copy, events.iter(), ctx)
    }
```

- [ ] **Step 3: Replace the obsolete webview test and add the new webview-leader tests in `src/input/resolve.rs`**

Delete the existing `webview_focus_clears_leader_and_forwards` test (it asserted the old reset-and-forward behavior, which is now replaced by leader precedence). Add these tests in the same `mod tests` (they use `test_shortcuts_with_repeat_prefix`, already imported at the top of the test module):

```rust
    #[test]
    fn webview_pending_leader_fires_action_and_suppresses() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyS, Key::Character("s".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            out.effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
                via_leader: true,
            }],
            "a bound second key fires its leader action even while a webview is focused"
        );
        assert_eq!(
            out.webview_suppressed,
            vec![KeyCode::KeyS],
            "the fired key is withheld from CEF"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn webview_idle_leader_chord_engages_and_suppresses() {
        // The fixture's leader is the Ctrl+A chord.
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let mut c = ctx(mods(true, false, false, false), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(out.effects, vec![], "the leader chord itself emits no effect");
        assert_eq!(
            out.webview_suppressed,
            vec![KeyCode::KeyA],
            "the leader chord is withheld from CEF"
        );
        assert_eq!(
            phase,
            LeaderPhase::Pending,
            "pressing the leader chord under webview focus engages Pending"
        );
    }

    #[test]
    fn webview_pending_unbound_key_swallowed_and_suppressed() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyZ, Key::Character("z".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(out.effects, vec![], "an unbound second key emits nothing");
        assert_eq!(
            out.webview_suppressed,
            vec![KeyCode::KeyZ],
            "the swallowed second key is still withheld from CEF"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn webview_idle_plain_key_not_suppressed() {
        let sc = Shortcuts::default();
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyB, Key::Character("b".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(out.effects, vec![], "a plain key under webview focus emits nothing");
        assert!(
            out.webview_suppressed.is_empty(),
            "a non-leader key is NOT withheld — the webview must still receive it"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn webview_idle_forward_chord_forwards_and_not_suppressed() {
        let sc = Shortcuts::default();
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let chords = [NormalizedChord {
            code: KeyCode::KeyK,
            alt: false,
            ctrl: false,
            shift: false,
            logo: false,
        }];
        let events = [press(KeyCode::KeyK, Key::Character("k".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        c.forward_chords = &chords;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            out.effects,
            vec![KeyEffect::WebviewForward {
                logical: Key::Character("k".into()),
                key_code: KeyCode::KeyK,
            }],
            "with no leader engaged a declared forward chord still forwards"
        );
        assert!(
            out.webview_suppressed.is_empty(),
            "a forward chord is not a leader claim — not withheld from CEF"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }
```

(The existing `webview_release_chord_emits_release` test is unchanged and still passes: `Idle` + the release chord returns `Passthrough` → `ReleaseWebviewFocus`.)

- [ ] **Step 4: Run the resolve.rs tests to verify Steps 1-3**

Run: `cargo test -p ozmux input::resolve::tests -- --nocapture`
Expected: PASS, including the five new `webview_*` tests. (`src/input/dispatch.rs` will NOT compile yet — that is Step 5. If cargo reports a build error in `dispatch.rs` before running resolve tests, that is expected; proceed to Step 5, then re-run.)

- [ ] **Step 5: Consume `ClassifiedKeys` and populate `CefKeyboardFilter` in `src/input/dispatch.rs`**

Change the bevy_cef import line from:

```rust
use bevy_cef::prelude::FocusedWebview;
```

to:

```rust
use bevy_cef::prelude::{CefKeyboardFilter, FocusedWebview, ModifiersState};
```

Change the `classify_key_batch` import to also bring the new type:

```rust
use crate::input::resolve::{BatchContext, ClassifiedKeys, KeyEffect, classify_key_batch};
```

Add a `CefKeyboardFilter` parameter to `resolve_shortcuts` (mutable params come first; place it next to the other `ResMut`s, after `focused_webview`):

```rust
    mut focused_webview: ResMut<FocusedWebview>,
    mut cef_filter: ResMut<CefKeyboardFilter>,
    mut leader_phase: ResMut<LeaderPhase>,
```

In the coarse-guard early-return block, clear the filter before returning. Change:

```rust
    if prompt_open || guards.ime.is_composing() || !focused_window {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }
```

to:

```rust
    if prompt_open || guards.ime.is_composing() || !focused_window {
        clear_leader_phase(&mut leader_phase);
        clear_cef_filter(&mut cef_filter);
        events.clear();
        return;
    }
```

Snapshot the focused webview entity BEFORE the effects loop that nulls it. The line `let focused = resolve_focused_surface(&focused_surface);` already sits before `classify_key_batch`; right after the `classify_key_batch` call, capture the entity and rename the result binding. Change:

```rust
    let all = classify_key_batch(
        &mut leader_phase,
        &inputs.shortcuts,
        &inputs.resolved_copy,
        events.read(),
        ctx,
    );

    let mut effects = Vec::with_capacity(all.len());
    for effect in all {
```

to:

```rust
    // Snapshot the webview entity BEFORE the effects loop below runs
    // `ReleaseWebviewFocus`, which sets `focused_webview.0 = None`. Building the
    // filter from `focused_webview.0` afterwards would drop the suppression on a
    // frame carrying both a leader claim and a release chord.
    let suppress_target = focused_webview.0;
    let ClassifiedKeys {
        effects: all,
        webview_suppressed,
    } = classify_key_batch(
        &mut leader_phase,
        &inputs.shortcuts,
        &inputs.resolved_copy,
        events.read(),
        ctx,
    );

    let mut effects = Vec::with_capacity(all.len());
    for effect in all {
```

After the effects loop and the `batch.write(...)` call, add the filter population. Insert this immediately after the `batch.write(ShortcutBatch { ... });` statement (at the end of the function body):

```rust
    let ms = ModifiersState {
        alt: mods.alt,
        ctrl: mods.ctrl,
        shift: mods.shift,
        logo: mods.meta,
    };
    match suppress_target {
        Some(webview) => cef_filter.set(
            webview_suppressed
                .into_iter()
                .map(|code| (webview, code, ms)),
        ),
        None => clear_cef_filter(&mut cef_filter),
    }
```

Add the private clear helper at the bottom of the file's non-test items (below `resolve_shortcuts`, above the `#[cfg(test)]` module):

```rust
/// Empties `CefKeyboardFilter`. Used on the coarse-guard early return and when no
/// webview owns the keyboard, so a stale leader claim never withholds a later key.
fn clear_cef_filter(cef_filter: &mut CefKeyboardFilter) {
    cef_filter.set(Vec::<(Entity, KeyCode, ModifiersState)>::new());
}
```

- [ ] **Step 6: Add `CefKeyboardFilter` to the test app and add the filter tests in `src/input/dispatch.rs`**

In the `resolve_app` test helper, add the resource init to the plugin/resource chain (next to the other `init_resource` calls):

```rust
            .init_resource::<CefKeyboardFilter>()
```

Add these tests to the `#[cfg(test)] mod tests` (they reuse the existing `resolve_app`, `press_key`, and `test_shortcuts_with_repeat_prefix` helpers):

```rust
    #[test]
    fn filter_holds_leader_claim_under_webview_focus() {
        let mut app = resolve_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            std::time::Duration::ZERO,
        ));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        press_key(&mut app, KeyCode::KeyS, Key::Character("s".into()));
        app.update();
        assert!(
            app.world().resource::<CefKeyboardFilter>().contains(
                webview,
                KeyCode::KeyS,
                ModifiersState::default()
            ),
            "the leader-claimed second key is withheld from CEF for the focused webview"
        );
    }

    #[test]
    fn filter_cleared_on_guarded_frame() {
        let mut app = resolve_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            std::time::Duration::ZERO,
        ));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        app.world_mut()
            .resource_mut::<CefKeyboardFilter>()
            .set([(webview, KeyCode::KeyS, ModifiersState::default())]);
        // Window unfocused → coarse guard fires.
        {
            let mut windows = app
                .world_mut()
                .query_filtered::<&mut Window, With<PrimaryWindow>>();
            windows.single_mut(app.world_mut()).unwrap().focused = false;
        }
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        press_key(&mut app, KeyCode::KeyS, Key::Character("s".into()));
        app.update();
        assert!(
            !app.world().resource::<CefKeyboardFilter>().contains(
                webview,
                KeyCode::KeyS,
                ModifiersState::default()
            ),
            "a guarded frame clears the filter so a stale claim never lingers"
        );
    }

    #[test]
    fn filter_cleared_when_nothing_claimed() {
        let mut app = resolve_app(Shortcuts::default());
        let term = app.world_mut().spawn((OzmaTerminal, KeyboardFocused)).id();
        let stale = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<CefKeyboardFilter>()
            .set([(stale, KeyCode::KeyS, ModifiersState::default())]);
        // No webview focused; a plain key claims nothing.
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert!(
            !app.world().resource::<CefKeyboardFilter>().contains(
                stale,
                KeyCode::KeyS,
                ModifiersState::default()
            ),
            "a frame that claims nothing clears the filter"
        );
        let _ = term;
    }
```

- [ ] **Step 7: Run the full affected test set**

Run: `cargo test -p ozmux input::`
Expected: PASS — the resolve.rs decider tests (including the five new `webview_*`) and the dispatch.rs tests (including the three new `filter_*`).

- [ ] **Step 8: Verify a clean build (no dead-code / unused warnings) and lint/format**

Run: `cargo build -p ozmux 2>&1 | grep -iE "warning|error" || echo "clean"`
Expected: `clean` (in particular, `webview_suppressed` is read in production by `resolve_shortcuts`, so no `dead_code`).

Run: `cargo clippy --workspace --all-targets && cargo fmt`
Expected: no clippy warnings; fmt applies no further changes on re-run.

- [ ] **Step 9: Commit**

```bash
git add src/input/resolve.rs src/input/dispatch.rs
git commit -m "$(cat <<'EOF'
feat(input): fire <Leader> shortcuts while a webview owns the keyboard

classify_key_batch now steps the leader machine on the webview-focused path
(returning ClassifiedKeys{effects, webview_suppressed}); resolve_shortcuts
withholds the leader-claimed keys from CEF via CefKeyboardFilter and clears it
on guarded / unclaimed frames.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Order filter population before CEF delivery

**Files:**
- Modify: `src/input/dispatch.rs` (imports ~26-28, `DispatchPlugin::build` ~60-77, tests)

**Interfaces:**
- Consumes: bevy_cef `KeyboardDeliverSet` (the `SystemSet` covering bevy_cef's keyboard-delivery systems, from `bevy_cef::prelude`).
- Produces: nothing new — adds a scheduling edge so `resolve_shortcuts` writes `CefKeyboardFilter` before `send_key_event` reads it.

- [ ] **Step 1: Add the `.before(KeyboardDeliverSet)` ordering edge in `src/input/dispatch.rs`**

Extend the bevy_cef import to include the set:

```rust
use bevy_cef::prelude::{CefKeyboardFilter, FocusedWebview, KeyboardDeliverSet, ModifiersState};
```

In `DispatchPlugin::build`, add `.before(KeyboardDeliverSet)` to the `resolve_shortcuts` registration. Change:

```rust
            .add_systems(
                Update,
                resolve_shortcuts
                    .in_set(ShortcutSet::Resolve)
                    .in_set(LeaderGate::Advance)
                    .run_if(on_message::<KeyboardInput>),
            );
```

to:

```rust
            .add_systems(
                Update,
                resolve_shortcuts
                    .in_set(ShortcutSet::Resolve)
                    .in_set(LeaderGate::Advance)
                    .before(KeyboardDeliverSet)
                    .run_if(on_message::<KeyboardInput>),
            );
```

- [ ] **Step 2: Add the ordering test in `src/input/dispatch.rs`**

This proves the guarantee's purpose: a probe registered in `KeyboardDeliverSet` observes the filter already populated, i.e. `resolve_shortcuts` ran first. Add to `mod tests`:

```rust
    #[derive(Resource, Default)]
    struct DeliverProbe {
        saw_claim: bool,
    }

    fn deliver_probe(
        mut probe: ResMut<DeliverProbe>,
        filter: Res<CefKeyboardFilter>,
        webview: Res<ProbeWebview>,
    ) {
        if let Some(webview) = webview.0 {
            probe.saw_claim = filter.contains(webview, KeyCode::KeyS, ModifiersState::default());
        }
    }

    #[derive(Resource, Default)]
    struct ProbeWebview(Option<Entity>);

    #[test]
    fn filter_is_populated_before_keyboard_deliver_set() {
        let mut app = resolve_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            std::time::Duration::ZERO,
        ));
        app.init_resource::<DeliverProbe>()
            .init_resource::<ProbeWebview>()
            .add_systems(Update, deliver_probe.in_set(KeyboardDeliverSet));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        app.world_mut().resource_mut::<ProbeWebview>().0 = Some(webview);
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        press_key(&mut app, KeyCode::KeyS, Key::Character("s".into()));
        app.update();
        assert!(
            app.world().resource::<DeliverProbe>().saw_claim,
            "resolve_shortcuts must populate CefKeyboardFilter before KeyboardDeliverSet runs"
        );
    }
```

- [ ] **Step 3: Run the ordering test**

Run: `cargo test -p ozmux input::dispatch::tests::filter_is_populated_before_keyboard_deliver_set`
Expected: PASS (no schedule-cycle panic; the probe observed the claim).

- [ ] **Step 4: Lint/format and full build**

Run: `cargo clippy --workspace --all-targets && cargo fmt && cargo build -p ozmux`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/input/dispatch.rs
git commit -m "$(cat <<'EOF'
feat(input): order resolve_shortcuts before bevy_cef KeyboardDeliverSet

Guarantees CefKeyboardFilter is populated in the same frame before
send_key_event reads it, so leader-claimed keys never leak into the page.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: End-to-end verification (human checkpoint)

**Files:** none (manual run).

- [ ] **Step 1: Confirm the `CefKeyboardFilter` resource exists at runtime**

Run: `cargo run -p ozmux` (requires `just setup-cef` provisioned). Confirm the app boots without a "resource does not exist: CefKeyboardFilter" panic — bevy_cef's `KeyboardPlugin` `init_resource`s it. If it panics because the resource is absent (e.g. a feature-gated build), defensively `init_resource::<CefKeyboardFilter>()` in `DispatchPlugin::build` and re-run; note this in the commit.

- [ ] **Step 2: Manually confirm the behavior**

With a mounted, focused inline webview (click into it so keyboard focus transfers to CEF):
1. Trigger a `<Leader>` shortcut (default: tap Cmd, then the bound key — e.g. `<Leader>` + copy-mode / new-window binding). Confirm the ozmux action fires.
2. Confirm the leader's second key does NOT appear typed into a focused web input on the page.
3. Confirm plain typing into the webview still reaches the page (no regression), and the release chord still blurs it.

Expected: leader shortcuts act on the pane/session behind the webview; no stray characters leak into the web content on the keydown.

(Optional: invoke the repo's `/verify` skill to drive the affected flow if a project verify harness exists.)

---

## Self-Review

**Spec coverage:**
- Part 1 (decider runs the leader during webview focus) → Task 1 Steps 1-3.
- Part 2 (populate/clear `CefKeyboardFilter`, `Vec<KeyCode>` shape, `Modifiers`→`ModifiersState` map, snapshot entity before the `ReleaseWebviewFocus` loop, clear on guard) → Task 1 Steps 5-6.
- Ordering `.before(KeyboardDeliverSet)` + no-cycle/ordering test → Task 2.
- Testing section (pure decider tests, applier/filter tests, schedule build) → Task 1 Steps 3/6, Task 2 Step 2.
- Non-goals (direct chords, forward_keys/release semantics, no blur) → preserved: the `Passthrough` arm keeps the exact release/forward handling and never calls `match_gui_action`, so direct chords stay webview-owned; no applier or focus change.
- Risks (batch.focused drift, key-release limitation, chord-leader modifiers reach CEF) → behavioral, no code required; end-to-end checks in Task 3.

**Placeholder scan:** none — every code step shows complete code and exact commands.

**Type consistency:** `ClassifiedKeys { effects: Vec<KeyEffect>, webview_suppressed: Vec<KeyCode> }` is defined in Task 1 Step 1 and destructured identically in Task 1 Step 5; `clear_cef_filter(&mut CefKeyboardFilter)` is defined and called (guard path + `None` arm) in Task 1 Step 5; `CefKeyboardFilter::set`/`contains` and `ModifiersState { alt, ctrl, shift, logo }` match the bevy_cef 0.11 API; `run`/`run_full` helper names are used consistently in Tasks 1-2.
