# Mode Change Connection Design

**Date:** 2026-06-19
**Scope:** ロードマップ項目 3 — モード変更の処理を繋ぎこむ

## Overview

`AppMode` Bevy state (`Ozma` | `Ozmux`) を `main.rs` に配線し、起動モード設定とデタッチショートカットを実装する。

- Config の `startup_mode` フィールドで起動時の `AppMode` を決定する
- `AppMode::Ozmux` への遷移でtmux接続を確立し、`AppMode::Ozma` への遷移で切断する
- デタッチショートカットで Ozmux → Ozma 遷移をトリガーできる

---

## Architecture

```
[起動時]
  OzmuxConfigs.startup_mode (ozma / ozmux / auto-attach)
    ↓
  main.rs → OzmaModePlugin::new(shell, initial_AppMode)
    ↓
  AppMode::Ozma  → OzmaModePlugin の OnEnter(Ozma) が単一ターミナルを spawn
  AppMode::Ozmux → OzmuxTmuxPlugin の OnEnter(Ozmux) がtmux接続 + TmuxPresence insert
                       └─ AutoAttachOnStartup なし → ピッカー表示
                       └─ AutoAttachOnStartup あり → 最後のセッションへ自動アタッチ

[デタッチ時]
  detach-session ショートカット → NextState(AppMode::Ozma)
    → OnExit(AppMode::Ozmux): detach-client 送信 → 接続クローズ → TmuxPresence 削除 → ECS entity 削除
    → OnEnter(AppMode::Ozma): 単一ターミナルを再 spawn
```

### クレート間依存方針

| レイヤー | ゲート方法 |
|---|---|
| `crates/tmux_session/` (`ozmux_tmux`) | `resource_exists::<TmuxPresence>()` |
| `src/tmux/` (UI サブプラグイン群) | `in_state(AppMode::Ozmux)` |
| `src/picker.rs` | `OnEnter(AppMode::Ozmux)` へ移動 |

`ozmux_tmux` クレートに `ozma_mode` 依存を追加しない。`TmuxPresence` の insert/remove を `OnEnter`/`OnExit(AppMode::Ozmux)` で行うことで `TmuxPresence ↔ AppMode::Ozmux` を常に同期させる。

---

## Section 1: Config Layer (`crates/configs/`)

### 新規ファイル `crates/configs/src/startup.rs`

```rust
pub enum StartupMode {
    Ozma,       // default
    Ozmux,      // セッションピッカーを表示
    AutoAttach, // 最後のtmuxセッションへ自動アタッチ
}

impl Default for StartupMode {
    fn default() -> Self { Self::Ozma }
}
```

`StartupMode` は `serde::Deserialize` を実装し、文字列 `"ozma"` / `"ozmux"` / `"auto-attach"` を受け付ける。

### `OzmuxConfigs` への追加

```rust
pub startup_mode: StartupMode,  // default = StartupMode::Ozma
```

### `raw.rs` の `RawConfigs` への追加

```rust
pub(crate) startup_mode: Option<StartupMode>,
```

`apply_to` 内で `if let Some(m) = self.startup_mode { base.startup_mode = m; }` を追加。

### `config.toml` 例

```toml
startup_mode = "ozmux"   # "ozma" | "ozmux" | "auto-attach"
```

---

## Section 2: `OzmaModePlugin` の変更 (`crates/ozma_mode/`)

### `OzmaModePlugin::new` シグネチャ

```rust
pub struct OzmaModePlugin {
    config_shell: Option<String>,
    initial_mode: AppMode,
}

impl OzmaModePlugin {
    pub fn new(config_shell: Option<String>, initial_mode: AppMode) -> Self {
        Self { config_shell, initial_mode }
    }
}
```

### `build` 内の変更

```rust
// before: app.init_state::<AppMode>()
app.insert_state(self.initial_mode.clone())
```

### `StartupMode` → `AppMode` マッピング（`main.rs` 側）

| `StartupMode` | `AppMode` |
|---|---|
| `Ozma` | `AppMode::Ozma` |
| `Ozmux` | `AppMode::Ozmux` |
| `AutoAttach` | `AppMode::Ozmux` |

`AutoAttach` か否かの区別は `AutoAttachOnStartup` マーカーリソース（`main.rs` で insert）が担う。

---

## Section 3: tmux 接続ライフサイクル

### `crates/tmux_session/` (`TmuxSessionPlugin`) の変更

- `build()` から `insert_resource(TmuxPresence)` を削除
- `drain_tmux_events` と `request_pane_captures` に `.run_if(resource_exists::<TmuxPresence>())` を追加

### `src/tmux.rs` (`OzmuxTmuxPlugin`) の追加システム

```rust
// OnEnter(AppMode::Ozmux)
fn on_enter_ozmux(mut commands: Commands, ...) {
    commands.insert_resource(TmuxPresence);
    // tmux -CC 接続を開始（attach_or_create）
    // startup_mode が AutoAttach の場合は直接最後のセッションにアタッチ
}

// OnExit(AppMode::Ozmux)
fn on_exit_ozmux(mut commands: Commands, connection: NonSend<TmuxConnection>, sessions: Query<Entity, With<TmuxSession>>, ...) {
    if let Some(client) = connection.client() {
        let _ = client.handle().send(&DetachClientCommand);
    }
    connection.close();
    commands.remove_resource::<TmuxPresence>();
    for entity in sessions.iter() {
        commands.entity(entity).despawn_recursive();
    }
}
```

`src/tmux/` 配下の UI サブプラグイン群（render, input, mouse, window_bar, copy_mode 等）の全 Update システムに `.run_if(in_state(AppMode::Ozmux))` を追加する。

---

## Section 4: ピッカー統合 (`src/picker.rs`)

- `list_sessions_into_picker` / `spawn_picker_ui` を `Startup` から `OnEnter(AppMode::Ozmux)` に移動
- `on_enter_ozmux` 内で `AutoAttachOnStartup` リソースの有無を確認：
  - **あり** → ピッカーを表示せず直接 `attach_or_create` を呼ぶ
  - **なし** → 従来通りピッカーを表示してユーザーが選択

---

## Section 5: デタッチショートカット

### `crates/configs/src/shortcuts.rs` への追加

```rust
pub detach_session: Option<ShortcutBinding>,  // デフォルト: 未設定（ユーザーが任意で設定）
```

### 処理系（`src/tmux/detach.rs` または `src/input/` 内）

```rust
// AppMode::Ozmux のときのみ動作
fn handle_detach_shortcut(
    mut next_state: ResMut<NextState<AppMode>>,
    configs: Res<OzmuxConfigsResource>,
    keys: ...,
) {
    if shortcut_triggered(&configs.shortcuts.bindings.detach_session, &keys) {
        next_state.set(AppMode::Ozma);
        // OnExit(AppMode::Ozmux) が自動的に切断・クリーンアップを担う
    }
}
```

`.run_if(in_state(AppMode::Ozmux))` でゲートする。

---

## Affected Files

| ファイル | 変更種別 |
|---|---|
| `crates/configs/src/startup.rs` | 新規 |
| `crates/configs/src/lib.rs` | `startup_mode` フィールド追加 |
| `crates/configs/src/raw.rs` | `startup_mode` フィールド追加 |
| `crates/ozma_mode/src/lib.rs` | `initial_mode` 引数追加、`insert_state` 切り替え |
| `crates/tmux_session/src/plugin.rs` | `TmuxPresence` insert 削除、`run_if` 追加 |
| `src/main.rs` | `ozma_mode` dep 追加、`OzmaModePlugin` 追加、`AutoAttachOnStartup` insert |
| `src/configs.rs` | startup_mode → AppMode 変換 |
| `src/tmux.rs` | `on_enter_ozmux` / `on_exit_ozmux` 追加 |
| `src/tmux/*.rs` | UI システムに `run_if(in_state(AppMode::Ozmux))` 追加 |
| `src/picker.rs` | `Startup` → `OnEnter(AppMode::Ozmux)` 移動、auto-attach 分岐 |
| `src/input/` or `src/tmux/detach.rs` | デタッチショートカット処理系 |
| `Cargo.toml` (root) | `ozma_mode = { path = "crates/ozma_mode" }` 追加 |
