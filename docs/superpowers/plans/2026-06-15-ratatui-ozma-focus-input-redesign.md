# ratatui-ozma Focus/Input Redesign (H: host-tunnel) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** webview にフォーカスがあっても、宣言したパススルーキーが `crossterm::event::read()` に `Event::Key` として届くようにし（host-tunnel）、SDK のフォーカス制御フレームワーク（FocusManager 等）を撤去してフォーカスをアプリ責務に戻す。

**Architecture:** ホストが「フォーカス webview の宣言パススルーキー」だけを PTY へ転送し、bevy_cef はそのキーを CEF へ送らない（二重配送抑止）。フォーカスは `WebviewWidget::focused(bool)` → flush 差分 → control-plane focus op で同期。クロスリポ: ozmux + ローカル bevy_cef worktree。

**Tech Stack:** Rust, Bevy 0.18, bevy_cef(local worktree `$HOME/workspace/bevy_cef/wt/passthrough` branch `passthrough`), ratatui 0.29(crossterm), `ozma_tty_engine`(VT エンコーダ), control-plane NDJSON。

**Spec:** `docs/superpowers/specs/2026-06-15-ratatui-ozma-focus-input-redesign-design.md`（論点・根拠の一次情報。本計画と併読）

**Conventions（`.claude/rules/`）:** no `mod.rs`; comments only `// TODO:`/`// NOTE:`/`// SAFETY:`; `///` on every `pub`; module `//!`; all `use` at top one block; mutable params first; private items last; English comments. bevy_cef worktree も同等の Rust 慣習。

**Build/test:**
- ozmux: `cargo test`（全体は並列で SIGSEGV しうる → `cargo test -p <crate> -- --test-threads=1`）。lint `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`。
- bevy_cef worktree: `cd $HOME/workspace/bevy_cef/wt/passthrough && cargo test`。

---

## File Structure（変更マップ）

**bevy_cef worktree（`$HOME/workspace/bevy_cef/wt/passthrough/`）**
- `src/keyboard.rs` — `CefKeyboardFilter` リソース(embedder 非依存) + `send_key_event`/`send_key_event_win` に抑止チェック + `KeyboardSystems` public SystemSet。

**ozmux — Cargo**
- `Cargo.toml`(root) — `[patch]` で bevy_cef/bevy_cef_core をローカルパスへ（dev 専用、コミットしない方針も検討）。

**ozmux — engine（`crates/ozma_tty_engine/`）**
- `src/input_codec.rs` — `encode_key` の Alt/Meta→ESC。

**ozmux — host（`src/`）**
- `input.rs` — passthrough 判定を shortcut lookup の前に hoist + PTY 転送 + bevy_cef filter 充填; `bevy_to_terminal_key` の Alt 基底文字復元(key_code 受け取り)。
- `control_plane/protocol.rs` / `control_plane.rs` — `RegisterKind`/`DynamicView`/`build_view` に `passthrough_keys`。
- `inline_webview.rs` — mount 時に正規化 passthrough を `InlineWebview` へ持たせる（focused child から即参照）。
- `webview_render.rs` — `sync_focused_webview` の権威ガート（app 宣言フォーカスを上書きしない）。

**ozmux — SDK（`sdk/ratatui-ozma/src/`）**
- 削除: `focus.rs`, `keymap.rs`。
- `keychord.rs`(新規) — `KeyChord { mods: KeyModifiers, code: KeyCode }` + 正規化 serialize。
- `webview.rs` — `Webview::passthrough`; `set_nav_keys`/`set_page_focus`/`focus`/`blur`/`focus_instance` 削除。
- `widget.rs` — `focused` を `FramePlacements` 記録へ。
- `session.rs` — flush の focus 差分送信（`ClientMsg::Focus` op は残置）。
- `protocol.rs` — `Register` に `passthrough` 追加。
- `lib.rs` — export 整理。

**ozmux — examples**
- `examples/focus_grid.rs`, `examples/ratatui_webview.rs` — 新 API + 素の `event::read()` + アプリ自前フォーカスリング。

---

## Phase 0 — bevy_cef をローカル worktree へ差し替え

### Task 0.1: `[patch]` で bevy_cef をローカルパスに

**Files:** Modify `Cargo.toml`(root)

- [ ] **Step 1: worktree の crate version/パスを確認**

Run:
```bash
cat "$HOME/workspace/bevy_cef/wt/passthrough/Cargo.toml" | grep -E '^name|^version' | head
ls "$HOME/workspace/bevy_cef/wt/passthrough/crates/bevy_cef_core/Cargo.toml"
```
Expected: `name = "bevy_cef"`, `version = "0.11.0-dev"`、bevy_cef_core の Cargo.toml が存在。

- [ ] **Step 2: root `Cargo.toml` 末尾に `[patch]` を追加**

```toml
# NOTE: dev-only local patch for the passthrough work; do not rely on this path in CI.
[patch."https://github.com/not-elm/bevy_cef"]
bevy_cef = { path = "/Users/taiga/workspace/bevy_cef/wt/passthrough" }
bevy_cef_core = { path = "/Users/taiga/workspace/bevy_cef/wt/passthrough/crates/bevy_cef_core" }
```

- [ ] **Step 3: patch が解決し build が通るか確認**

Run: `cargo metadata --format-version 1 2>&1 | grep -c 'bevy_cef/wt/passthrough'`
Expected: `> 0`（path patch が効いている）。
Run: `cargo build -p ozmux-gui 2>&1 | tail -3`
Expected: `Finished`（version 不一致なら patch が無効化される → worktree の version を `headless-gpu` に合わせる）。

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: [patch] bevy_cef to local passthrough worktree (dev)"
```

---

## Phase 1 — bevy_cef: キー抑止フック（worktree 内）

作業ディレクトリは `$HOME/workspace/bevy_cef/wt/passthrough`。コミットも当該リポで。

### Task 1.1: `CefKeyboardFilter` リソースと SystemSet

**Files:** Modify `$HOME/workspace/bevy_cef/wt/passthrough/src/keyboard.rs`

- [ ] **Step 1: 抑止リソース + public SystemSet を定義**

`keyboard.rs` の `use` 群直後に追加:
```rust
/// Embedder-provided filter: keys for which the focused webview must NOT receive
/// CEF delivery this frame (the embedder routes them elsewhere, e.g. to a PTY).
/// Keyed by the focused webview entity + the Bevy physical key + modifiers.
#[derive(Resource, Default)]
pub struct CefKeyboardFilter {
    suppressed: std::collections::HashSet<(Entity, KeyCode, ModifiersState)>,
}

/// Modifier snapshot a [`CefKeyboardFilter`] entry carries (Bevy-side).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ModifiersState {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub logo: bool,
}

impl CefKeyboardFilter {
    /// Replaces the suppressed set (embedder fills this each frame before keyboard delivery).
    pub fn set(&mut self, entries: impl IntoIterator<Item = (Entity, KeyCode, ModifiersState)>) {
        self.suppressed = entries.into_iter().collect();
    }
    /// Whether `(webview, code, mods)` should be withheld from CEF this frame.
    pub fn contains(&self, webview: Entity, code: KeyCode, mods: ModifiersState) -> bool {
        self.suppressed.contains(&(webview, code, mods))
    }
}

/// Public system set for the keyboard-delivery systems, so embedders can order
/// their `CefKeyboardFilter` population `.before(KeyboardSystems::Deliver)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyboardDeliverSet;
```
（`ModifiersState` は既存 `keyboard_modifiers` の戻り型に合わせる。実型を確認し、`bevy_cef_core` の modifier 型を再利用できればそれを使う。）

- [ ] **Step 2: `KeyboardPlugin::build` で登録**

`build` 冒頭の `init_resource` 群に追加:
```rust
app.init_resource::<CefKeyboardFilter>();
```
非Windows/Windows の `add_systems(Update, (...).chain())` の対象システム集合に `.in_set(KeyboardDeliverSet)` を付与（`send_key_event`/`send_key_event_win` を含むタプルに `.in_set(KeyboardDeliverSet)`）。

- [ ] **Step 3: `send_key_event` に抑止チェック**

`send_key_event`(line 104-140) の引数に `filter: Res<CefKeyboardFilter>` を追加。`let Some(webview) = target else { continue; };` の直後に:
```rust
let ms = ModifiersState { alt: input.pressed(KeyCode::AltLeft) || input.pressed(KeyCode::AltRight),
    ctrl: input.pressed(KeyCode::ControlLeft) || input.pressed(KeyCode::ControlRight),
    shift: input.pressed(KeyCode::ShiftLeft) || input.pressed(KeyCode::ShiftRight),
    logo: input.pressed(KeyCode::SuperLeft) || input.pressed(KeyCode::SuperRight) };
if filter.contains(webview, event.key_code, ms) {
    continue; // embedder routes this key elsewhere (e.g. PTY); do not deliver to CEF.
}
```
`send_key_event_win`(line 197-229) にも同一の追加。

- [ ] **Step 4: 単体テスト**（`keyboard.rs` の `#[cfg(test)] mod tests`、無ければ新設）

```rust
#[test]
fn filter_contains_matches_entry() {
    let mut f = CefKeyboardFilter::default();
    let e = Entity::from_raw(7);
    let ms = ModifiersState { alt: true, ..Default::default() };
    f.set([(e, KeyCode::KeyH, ms)]);
    assert!(f.contains(e, KeyCode::KeyH, ms));
    assert!(!f.contains(e, KeyCode::KeyH, ModifiersState::default()));
    assert!(!f.contains(Entity::from_raw(8), KeyCode::KeyH, ms));
}
```

- [ ] **Step 5: build/test**

Run: `cd "$HOME/workspace/bevy_cef/wt/passthrough" && cargo test --lib keyboard 2>&1 | tail -5`
Expected: PASS。（CEF feature が要る場合は `--features ...` を確認。）

- [ ] **Step 6: Commit（bevy_cef リポ）**

```bash
cd "$HOME/workspace/bevy_cef/wt/passthrough"
git add src/keyboard.rs && git commit -m "feat(keyboard): CefKeyboardFilter to suppress embedder-routed keys from CEF"
```

---

## Phase 2 — エンジン: Alt/Meta 符号化

### Task 2.1: `encode_key` の Alt/Meta→ESC

**Files:** Modify `crates/ozma_tty_engine/src/input_codec.rs`

- [ ] **Step 1: 失敗するテスト**（`mod tests`）

```rust
fn alt() -> TerminalModifiers { TerminalModifiers { alt: true, ..Default::default() } }

#[test]
fn alt_letter_is_esc_prefixed() {
    assert_eq!(encode_key(&TerminalKey::Text("h".into()), &alt(), false), Some(b"\x1bh".to_vec()));
}
#[test]
fn meta_letter_is_esc_prefixed() {
    let meta = TerminalModifiers { meta: true, ..Default::default() };
    assert_eq!(encode_key(&TerminalKey::Text("x".into()), &meta, false), Some(b"\x1bx".to_vec()));
}
#[test]
fn alt_does_not_double_prefix_escape_or_break_ctrl() {
    // Ctrl takes priority (control byte), Alt+Ctrl handled by ctrl branch.
    let ctrl = TerminalModifiers { ctrl: true, ..Default::default() };
    assert_eq!(encode_key(&TerminalKey::Text("a".into()), &ctrl, false), Some(vec![0x01]));
}
```

- [ ] **Step 2: Run → FAIL**

Run: `cargo test -p ozma_tty_engine alt_letter_is_esc_prefixed`
Expected: FAIL（Alt ブランチ無し → ESC が付かない）。

- [ ] **Step 3: `encode_key` に Alt/Meta ブランチ**

`encode_key`(input_codec.rs:18-42) の最初の Ctrl 分岐の後・arrow の前に、base バイト生成を helper 化して ESC を前置:
```rust
// meta-sends-escape: Alt/Meta on a Text key emits ESC + the key bytes.
if (mods.alt || mods.meta)
    && let TerminalKey::Text(s) = key
    && !s.is_empty()
{
    let mut out = vec![0x1b];
    out.extend_from_slice(s.as_bytes());
    return Some(out);
}
```
（arrows/special への modifier は CSI エンコーディングが本来必要だが、本対応のスコープは Text キーの meta-sends-escape のみ。スコープ外は spec の注記参照。）

- [ ] **Step 4: Run → PASS + 既存回帰**

Run: `cargo test -p ozma_tty_engine --lib input_codec`
Expected: PASS（既存の printable/ctrl/arrow/special テストも green）。

- [ ] **Step 5: Commit**

```bash
git add crates/ozma_tty_engine/src/input_codec.rs
git commit -m "feat(engine): encode_key emits ESC prefix for Alt/Meta Text keys (meta-sends-escape)"
```

### Task 2.2: `bevy_to_terminal_key` の Alt 基底文字復元

**Files:** Modify `src/input.rs`

- [ ] **Step 1: 失敗するテスト**（`src/input.rs` の `mod tests`）

`bevy_to_terminal_key` を「Alt 時は physical key_code から基底文字」に変える。現状 `bevy_to_terminal_key(key: &Key)`。新シグネチャ `bevy_to_terminal_key(key: &Key, key_code: KeyCode, alt: bool)`。テスト:
```rust
#[test]
fn alt_recovers_base_letter_from_key_code() {
    use bevy::input::keyboard::Key as Bk;
    // logical_key が合成グリフでも、Alt 時は key_code(KeyH) から 'h' を復元
    let tk = bevy_to_terminal_key(&Bk::Character("˙".into()), KeyCode::KeyH, true);
    assert!(matches!(tk, Some(TerminalKey::Text(ref s)) if s == "h"));
}
#[test]
fn non_alt_uses_logical_key() {
    use bevy::input::keyboard::Key as Bk;
    let tk = bevy_to_terminal_key(&Bk::Character("h".into()), KeyCode::KeyH, false);
    assert!(matches!(tk, Some(TerminalKey::Text(ref s)) if s == "h"));
}
```

- [ ] **Step 2: Run → FAIL**（シグネチャ不一致）

Run: `cargo test -p ozmux-gui alt_recovers_base_letter_from_key_code`
Expected: FAIL（引数不一致 / 復元なし）。

- [ ] **Step 3: 実装**

`bevy_to_terminal_key`(input.rs:482) を:
```rust
fn bevy_to_terminal_key(key: &Key, key_code: KeyCode, alt: bool) -> Option<TerminalKey> {
    if alt && let Some(c) = base_char_from_key_code(key_code) {
        return Some(TerminalKey::Text(c.to_string()));
    }
    Some(match key { /* 既存の match のまま */ })
}

/// US-layout base char for a physical key when a modifier composed the logical key.
fn base_char_from_key_code(code: KeyCode) -> Option<char> {
    use KeyCode::*;
    Some(match code {
        KeyA => 'a', KeyB => 'b', KeyC => 'c', KeyD => 'd', KeyE => 'e', KeyF => 'f',
        KeyG => 'g', KeyH => 'h', KeyI => 'i', KeyJ => 'j', KeyK => 'k', KeyL => 'l',
        KeyM => 'm', KeyN => 'n', KeyO => 'o', KeyP => 'p', KeyQ => 'q', KeyR => 'r',
        KeyS => 's', KeyT => 't', KeyU => 'u', KeyV => 'v', KeyW => 'w', KeyX => 'x',
        KeyY => 'y', KeyZ => 'z',
        Digit0 => '0', Digit1 => '1', Digit2 => '2', Digit3 => '3', Digit4 => '4',
        Digit5 => '5', Digit6 => '6', Digit7 => '7', Digit8 => '8', Digit9 => '9',
        _ => return None,
    })
}
```
呼び出し側(input.rs:248)は `bevy_to_terminal_key(&ev.logical_key, ev.key_code, mods.alt)` に更新（Phase 4 の passthrough 転送でも同関数を使う）。

- [ ] **Step 4: Run → PASS + 既存回帰**

Run: `cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 input::`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/input.rs
git commit -m "feat(input): bevy_to_terminal_key recovers base char from key_code under Alt"
```

---

## Phase 3 — control-plane / SDK protocol: passthrough 宣言

### Task 3.1: SDK `KeyChord` 型（新規 `keychord.rs`）

**Files:** Create `sdk/ratatui-ozma/src/keychord.rs`; Modify `sdk/ratatui-ozma/src/lib.rs`

- [ ] **Step 1: 型 + serialize テスト**

`keychord.rs`:
```rust
//! A keyboard chord declared as passthrough for a webview (crossterm-typed).

use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde::Serialize;
use serde::ser::SerializeMap;

/// A modifier chord the webview lets through to the app while focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyChord {
    /// Required modifiers.
    pub mods: KeyModifiers,
    /// The key code.
    pub code: KeyCode,
}

impl Serialize for KeyChord {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Wire shape consumed by the host: { "mods": [...], "key": "..." }.
        let mut mods = Vec::new();
        if self.mods.contains(KeyModifiers::ALT) { mods.push("alt"); }
        if self.mods.contains(KeyModifiers::CONTROL) { mods.push("ctrl"); }
        if self.mods.contains(KeyModifiers::SHIFT) { mods.push("shift"); }
        if self.mods.contains(KeyModifiers::SUPER) { mods.push("meta"); }
        let key = match self.code {
            KeyCode::Char(c) => c.to_ascii_lowercase().to_string(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "backtab".into(),
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_lowercase(),
        };
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("mods", &mods)?;
        m.serialize_entry("key", &key)?;
        m.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn serializes_to_wire_shape() {
        let c = KeyChord { mods: KeyModifiers::ALT, code: KeyCode::Char('H') };
        assert_eq!(serde_json::to_value(c).unwrap(), json!({"mods":["alt"],"key":"h"}));
    }
}
```
`lib.rs`: `mod keychord;` + `pub use keychord::KeyChord;`。

- [ ] **Step 2: Run → PASS**

Run: `cargo test -p ratatui-ozma --lib keychord`
Expected: PASS。

- [ ] **Step 3: Commit**

```bash
git add sdk/ratatui-ozma/src/keychord.rs sdk/ratatui-ozma/src/lib.rs
git commit -m "feat(ratatui-ozma): KeyChord type with host wire serialization"
```

### Task 3.2: SDK `Webview::passthrough` + register wire

**Files:** Modify `sdk/ratatui-ozma/src/webview.rs`, `sdk/ratatui-ozma/src/protocol.rs`

- [ ] **Step 1: protocol に passthrough を載せる**

SDK `protocol.rs` の `RegisterKind`(Inline/Dir) に `#[serde(skip_serializing_if = "Vec::is_empty")] passthrough: Vec<KeyChord>` を追加（両 variant）。`use crate::keychord::KeyChord;` を追加。

- [ ] **Step 2: `Webview::passthrough` ビルダ + テスト**

`webview.rs` の `Webview` に内部 `passthrough: Vec<KeyChord>` を持たせ、`inline`/`dir` 構築時に空で初期化、ビルダ:
```rust
/// Declares chords the page lets through to the app while focused.
pub fn passthrough(mut self, keys: impl IntoIterator<Item = KeyChord>) -> Self {
    self.passthrough.extend(keys);
    self
}
```
`register` 送出時に `RegisterKind` の `passthrough` へ流す。テスト: `Webview::inline("x").passthrough([KeyChord{..}])` の register 行 JSON に `"passthrough":[{"mods":["alt"],"key":"h"}]` が載る（pair-socket で `register` 送出をキャプチャ、既存 test 流儀）。

- [ ] **Step 3: Run → PASS / Step 4: Commit**

```bash
cargo test -p ratatui-ozma --lib webview
git add sdk/ratatui-ozma/src/webview.rs sdk/ratatui-ozma/src/protocol.rs
git commit -m "feat(ratatui-ozma): Webview::passthrough declares passthrough chords at register"
```

### Task 3.3: host control-plane に passthrough を受理・保持

**Files:** Modify `src/control_plane/protocol.rs`, `src/control_plane.rs`, `src/inline_webview.rs`

- [ ] **Step 1: wire 受理テスト**（`control_plane/protocol.rs` tests）

`RegisterKind`(Inline/Dir) に `#[serde(default)] passthrough: Vec<HostKeyChord>` を追加。`HostKeyChord { mods: Vec<String>, key: String }`(Deserialize)。テスト:
```rust
#[test]
fn parses_register_with_passthrough() {
    let m: ClientMsg = serde_json::from_str(
        r#"{"op":"register","kind":"inline","html":"x","passthrough":[{"mods":["alt"],"key":"h"}]}"#).unwrap();
    // assert passthrough length 1, key "h", mods ["alt"]
}
```

- [ ] **Step 2: `DynamicView` + `build_view` に正規化保持**

`control_plane.rs` の `DynamicView`(struct, line 35-52) に `passthrough: Vec<NormalizedChord>` を追加。`NormalizedChord { code: bevy::KeyCode, alt: bool, ctrl: bool, shift: bool, logo: bool }`（CEF 抑止・PTY 照合で使う host 内部表現。型正規化、spec §E）。`build_view`(line 511) で `HostKeyChord → NormalizedChord` 変換（"h"→`KeyCode::KeyH`, "tab"→`Tab`, "f5"→`F5`, mods 文字列→bool）。変換 helper をテスト。

- [ ] **Step 3: mount 時に `InlineWebview` へ resolved copy**

`inline_webview.rs` の `InlineWebview` component（or 並走 component `PassthroughKeys(Vec<NormalizedChord>)`）に、mount 時 `DynamicView.passthrough` をコピー（focused child から registry lookup なしで参照するため。spec §C 最適化）。`mount_inline` で挿入。

- [ ] **Step 4: Run → PASS / Step 5: Commit**

```bash
cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 control_plane
git add src/control_plane/protocol.rs src/control_plane.rs src/inline_webview.rs
git commit -m "feat(control-plane): accept + normalize passthrough_keys into DynamicView/InlineWebview"
```

---

## Phase 4 — host 入力配線: パススルー転送 + 抑止充填 + 権威ガート

### Task 4.1: bevy_cef filter を毎フレーム充填

**Files:** Modify `src/input.rs`（or a small new system）

- [ ] **Step 1: 充填システム**

フォーカス中 inline webview の `PassthroughKeys` から `(Entity, KeyCode, ModifiersState)` を作り `CefKeyboardFilter.set(...)` する system を追加（`bevy_cef::CefKeyboardFilter` を `ResMut`）。フォーカスが無い/非 webview なら空 set。**順序**: `KeyboardDeliverSet`(bevy_cef) の `.before()` に置く:
```rust
app.add_systems(Update, fill_cef_keyboard_filter.before(bevy_cef::prelude::KeyboardDeliverSet));
```
（`bevy_cef` から `KeyboardDeliverSet`/`CefKeyboardFilter`/`ModifiersState` を import。Phase 1 で export 済み。）

- [ ] **Step 2: テスト**

`make_test_app` 流儀で、フォーカス webview + passthrough を持つ child を用意 → system 実行 → `CefKeyboardFilter.contains(child, KeyH, alt)` が true。非フォーカスなら空。

- [ ] **Step 3: Run/Commit**

```bash
cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 fill_cef_keyboard_filter
git add src/input.rs && git commit -m "feat(input): populate bevy_cef CefKeyboardFilter from focused webview passthrough"
```

### Task 4.2: passthrough を shortcut lookup の前に hoist して PTY 転送

**Files:** Modify `src/input.rs`（`dispatch_focused_key`）

- [ ] **Step 1: テスト（passthrough は forward / 非passthrough は suppress / shortcut より優先）**

`dispatch_focused_key` のテスト（既存 `make_app` + `CapturedKeys` 流儀）:
- フォーカス webview + `Alt+h` ∈ passthrough → `TerminalKeyInput` が1つ捕捉される（PTY forward）。
- フォーカス webview + `Alt+x`(非passthrough) → forward されない（CEF のみ）。
- `Alt+h` が同時に ozmux ショートカットでも passthrough が勝つ（forward される）。

- [ ] **Step 2: 実装（line 222、shortcut lookup 223 の直前に挿入）**

`is_modifier_only_key` 判定(219-221)の直後、shortcut lookup(223)の前に:
```rust
// Passthrough: a focused webview's declared chord goes to the PTY (the app
// reads it via crossterm) and is suppressed from CEF (CefKeyboardFilter).
// Hoisted ABOVE shortcut lookup so a declared chord wins over a global shortcut.
if let Some(child) = focused_inline
    && passthrough_matches(child, &passthrough_q, ev.key_code, &mods)
{
    if !ev.repeat
        && let Some(tk) = bevy_to_terminal_key(&ev.logical_key, ev.key_code, mods.alt)
    {
        forward_to_active_terminal(&mut commands, &mux, workspace, tk,
            shortcut_mods_to_terminal_mods(&mods));
    }
    continue;
}
```
`passthrough_matches` は focused child の `PassthroughKeys` component（`passthrough_q: Query<&PassthroughKeys>`）を引いて `(key_code, mods)` 照合。`dispatch_focused_key` の引数に `passthrough_q` を追加。

- [ ] **Step 3: Run → PASS + 既存回帰 / Step 4: Commit**

```bash
cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 input::
git add src/input.rs && git commit -m "feat(input): forward focused-webview passthrough chords to PTY (above shortcut lookup)"
```

### Task 4.3: `sync_focused_webview` の権威ガート

**Files:** Modify `src/webview_render.rs`

- [ ] **Step 1: テスト（app 宣言フォーカスを sync が上書きしない）**

`sync_focused_webview`(webview_render.rs:91-112) のテスト: アプリ宣言フォーカス（control-plane focus op 経由で `FocusedWebview` が active surface の inline child に設定）が、その child が active surface の子である限り、毎フレーム sync 後も保持される。既に line 104-106 で focused inline child は preserve されているはず → そのアームが app 宣言にも効くことを確認。効かないケース（宣言先が active-pane 由来 target と異なる）があれば、宣言フォーカスを示すマーカー（例 `AppDeclaredFocus(Entity)` resource、flush 適用時にセット）を見て上書きを抑止するアームを追加。

- [ ] **Step 2: 実装**

`apply_control_events` の Focus 適用（control_plane.rs:456-468）で `FocusedWebview` を set する際、`AppDeclaredFocus` リソース（新規 or 既存マーカー）も更新。`sync_focused_webview` は `AppDeclaredFocus` が指す entity を `focused_inline_of` と同様に preserve する分岐を追加（既存の inline-child-preserve アームの条件を拡張）。

- [ ] **Step 3: Run → PASS / Step 4: Commit**

```bash
cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 webview_render sync_focused
git add src/webview_render.rs src/control_plane.rs && git commit -m "fix(webview-render): preserve app-declared focus against per-frame sync"
```

---

## Phase 5 — SDK 一掃 + widget 駆動フォーカス

### Task 5.1: `WebviewWidget::focused` を FramePlacements に記録

**Files:** Modify `sdk/ratatui-ozma/src/widget.rs`, `sdk/ratatui-ozma/src/session.rs`

- [ ] **Step 1: テスト**

`FramePlacements` に `focused: Option<String>`（このフレームで focused な handle）を持たせ、`WebviewWidget::render` が `focused==true` の時に `state.set_focused(handle)` を呼ぶ。テスト: focused(true) で render → `frame.focused_for_test() == Some("v")`。複数 focused は最後勝ち + `debug_assert!`。

- [ ] **Step 2: 実装**

`widget.rs` の `render`(line 56-63、現在 `state.record(handle, area)` のみ)に:
```rust
state.record(self.handle.to_owned(), area);
if self.focused {
    state.set_focused(self.handle.to_owned());
}
```
`session.rs` の `FramePlacements` に `focused: Option<String>` + `set_focused`/clear（`Ozma::frame()` で natives 同様 clear）。

- [ ] **Step 3: Run/Commit**

```bash
cargo test -p ratatui-ozma --lib widget session
git add sdk/ratatui-ozma/src/widget.rs sdk/ratatui-ozma/src/session.rs
git commit -m "feat(ratatui-ozma): WebviewWidget::focused records focused handle per frame"
```

### Task 5.2: `Ozma::flush` が focus 差分で Focus op 送信

**Files:** Modify `sdk/ratatui-ozma/src/session.rs`

- [ ] **Step 1: テスト**

`FlushState` に `last_focused: Option<String>` を追加。`flush` で「今フレームの focused が last と異なる時のみ」`ClientMsg::Focus{ handle: Some(h)/None }` を送る。テスト（pair-socket、既存 flush テスト流儀）: focused 変化時に `op:focus` 行が出る、不変なら出ない、None になると blur。

- [ ] **Step 2: 実装**

`flush_placements`（or `flush` 本体）に focus 差分送信を追加（`ClientMsg::Focus` は既存・残置）。`WebviewHandle::focus`/`blur` は削除するので、flush は `ClientMsg::Focus` を直接 serialize して writer に書く。

- [ ] **Step 3: Run/Commit**

```bash
cargo test -p ratatui-ozma --lib session
git add sdk/ratatui-ozma/src/session.rs
git commit -m "feat(ratatui-ozma): flush emits Focus op on focused-widget change"
```

### Task 5.3: FocusManager/keymap/handle-API のハード全廃

**Files:** Delete `sdk/ratatui-ozma/src/focus.rs`, `keymap.rs`; Modify `webview.rs`, `lib.rs`

- [ ] **Step 1: 削除と export 整理**

- `rm sdk/ratatui-ozma/src/focus.rs sdk/ratatui-ozma/src/keymap.rs`。
- `lib.rs`: `mod focus; mod keymap;` と `pub use focus::{...}; pub use keymap::{...};` を削除。`pub use keychord::KeyChord;` は残す。
- `webview.rs`: `WebviewHandle::{set_nav_keys, set_page_focus, focus, focus_instance, send_focus}` と `Webview::on_reserved`/`focusable` 連携を削除。`Ozma::blur`(session.rs) も削除。`Direction` 参照箇所を除去。
- `widget.rs`: `Direction` import が残っていれば除去。

- [ ] **Step 2: build → 壊れた参照を解消**

Run: `cargo build -p ratatui-ozma 2>&1 | grep -E "error" | head`
壊れた参照（examples 除く lib）をすべて解消。`ClientMsg::Focus` は protocol に残す（flush が使用）。

- [ ] **Step 3: lib テスト green**

Run: `cargo test -p ratatui-ozma --lib`
Expected: PASS（残った protocol/webview/widget/session/keychord/handler/error/osc テスト）。

- [ ] **Step 4: Commit**

```bash
git add -A sdk/ratatui-ozma/src
git commit -m "refactor(ratatui-ozma): hard-remove FocusManager/NavKeymap/focus/blur (focus is app-owned)"
```

---

## Phase 6 — examples 書き換え

### Task 6.1: `ratatui_webview.rs` を新 API へ

**Files:** Modify `sdk/ratatui-ozma/examples/ratatui_webview.rs`

- [ ] **Step 1: 書換**

- `FocusManager`/`NavKeymap`/`focusable` を撤去。`Webview::inline(html).passthrough([KeyChord{mods:ALT,code:Char('h')}, KeyChord{mods:ALT,code:Char('l')}]).on("ping",...)`。
- アプリ側に `let mut web_focused = false;` の単純フォーカス。ループは**素の `event::read()` のみ**（パススルーも crossterm 経由で来る）:
```rust
if event::poll(Duration::from_millis(50))? && let Event::Key(k) = event::read()? {
    match (k.modifiers, k.code) {
        (KeyModifiers::ALT, KeyCode::Char('l')) => web_focused = true,
        (KeyModifiers::ALT, KeyCode::Char('h')) => web_focused = false,
        (KeyModifiers::NONE, KeyCode::Char('q')) if !web_focused => return Ok(()),
        _ => {}
    }
}
```
- 描画は `WebviewWidget::new(view.id()).focused(web_focused)`、`ozma.flush` が同期。

- [ ] **Step 2: build / Step 3: Commit**

```bash
cargo build -p ratatui-ozma --example ratatui_webview
git add sdk/ratatui-ozma/examples/ratatui_webview.rs
git commit -m "docs(ratatui-ozma): ratatui_webview uses passthrough + plain event::read"
```

### Task 6.2: `focus_grid.rs` を新 API + 自前フォーカスリングへ

**Files:** Modify `sdk/ratatui-ozma/examples/focus_grid.rs`

- [ ] **Step 1: 書換**

2x2 グリッド。アプリが `focused: &str`（"nw"/"ne"/"sw"/"se"）でフォーカスリングを自実装（矩形から方向移動を自前計算 or 単純な順送り）。webview セルは `passthrough([Alt+hjkl])` で register。素の `event::read()` で `Alt+hjkl` を受けて `focused` を更新。各 webview セルは `WebviewWidget::focused(focused==id)`。FOCUS 表示・枠ハイライト。

- [ ] **Step 2: build / clippy / Step 3: Commit**

```bash
cargo build -p ratatui-ozma --example focus_grid
cargo clippy -p ratatui-ozma --all-targets 2>&1 | grep -E "warning|error" | head
git add sdk/ratatui-ozma/examples/focus_grid.rs
git commit -m "docs(ratatui-ozma): focus_grid uses app-owned focus ring + passthrough"
```

---

## Phase 7 — 全体検証

### Task 7.1: ワークスペース検証 + lint

- [ ] **Step 1: SDK + host テスト**

Run: `cargo test -p ratatui-ozma && cargo test -p ozma_tty_engine && cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 2>&1 | grep "test result"`
Expected: 全 PASS（並列全体は SIGSEGV しうるので single-thread）。

- [ ] **Step 2: bevy_cef worktree テスト**

Run: `cd "$HOME/workspace/bevy_cef/wt/passthrough" && cargo test 2>&1 | grep "test result"`
Expected: PASS。

- [ ] **Step 3: clippy + fmt**

Run: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt && cargo test -p ratatui-ozma`
Expected: 警告0、green。

- [ ] **Step 4: 実機手動（ペイン内 `cargo run`）**

`cargo run -p ratatui-ozma --example focus_grid` を ozmux ペイン内で実行（patch 済み ozmux で）:
- webview フォーカス中に `Alt+h/l` が**フォーカス移動に効く**（`event::read` 到達）。
- そのキーは**ページ内 textarea に入らない**（bevy_cef 抑止）。
- 宣言外キー（通常入力）は**ページへ**届く。
- `q` はネイティブフォーカス時のみ終了。

- [ ] **Step 5: Commit（lint fixups）**

```bash
git add -A && git commit -m "chore: clippy/fmt fixups for focus/input redesign"
```

---

## Notes for the implementer
- **クロスリポ commit**: bevy_cef の変更は `$HOME/workspace/bevy_cef/wt/passthrough` リポでコミット。ozmux の変更は ratatui-focus ブランチ。`[patch]` の path はコミットするが CI 非再現の注記（spec §B）。
- **型の3空間**（crossterm/bevy/terminal）: SDK=crossterm、host 正規化=`NormalizedChord`(bevy KeyCode)、PTY=TerminalKey。register 時に1度だけ正規化（Task 3.3）。
- **precedence**: passthrough は shortcut lookup より前（Task 4.2）。**権威**: app 宣言フォーカスは `sync_focused_webview` に上書きされないよう保護（Task 4.3）。
- **Ctrl+letter 曖昧性**（`Ctrl+h=0x08`）、**macOS 非US配列**、**クリック→アプリ双方向同期**は spec のスコープ外。
- bevy_cef の `ModifiersState`/`keyboard_modifiers` 実型・CEF feature 要否は worktree を読んで確定（Task 1.1 で実型に合わせる）。
