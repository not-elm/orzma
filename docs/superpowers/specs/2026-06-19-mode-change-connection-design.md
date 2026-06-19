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
  AppMode::Ozmux → OzmuxTmuxPlugin の OnEnter(Ozmux) が TmuxPresence insert + ピッカー/auto-attach を判定
                       └─ StartupMode::Ozmux → SessionPicker.open = true でピッカー表示
                             ユーザーが選択 → picker の handle_picker_input が attach_or_create を呼ぶ
                       └─ StartupMode::AutoAttach → select_attach_target() で最善セッションを選び attach_or_create を直接呼ぶ

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
#[derive(Deserialize, Default, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    #[default]
    Ozma,
    Ozmux,
    AutoAttach,
}
```

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

`AutoAttach` か否かの区別は `on_enter_ozmux` 内で `Res<OzmuxConfigsResource>` から `startup_mode` を直接読んで判定する。マーカーリソースは導入しない。

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
fn on_exit_ozmux(mut commands: Commands, mut connection: NonSendMut<TmuxConnection>, ...) {
    if let Some(client) = connection.client() {
        let _ = client.handle().send("detach-client");
    }
    connection.take();
    commands.remove_resource::<TmuxPresence>();
    // TmuxConnectionReset を trigger して既存の on_connection_reset observer がクリーンアップを担う
    // (TmuxConnectionReset は ozmux_tmux から pub 再エクスポートが必要)
    commands.trigger(TmuxConnectionReset);
}
```

`src/tmux/` 配下の UI サブプラグイン群（render, input, mouse, window_bar, copy_mode 等）の全 Update システムに `.run_if(in_state(AppMode::Ozmux))` を追加する。

**注意:** `src/picker.rs`、`src/tmux/dialog.rs`、`src/tmux/window_bar.rs` には `Startup` / `PostStartup` スケジュールのシステムがある。これらは `run_if(in_state(AppMode::Ozmux))` を付けるか、`OnEnter(AppMode::Ozmux)` に移動する（Section 4 参照）。

---

## Section 4: ピッカー統合 (`src/picker.rs`)

- `list_sessions_into_picker` / `spawn_picker_ui` を `Startup` から `OnEnter(AppMode::Ozmux)` に移動（重複エンティティを防ぐため再スポーンせず `SessionPicker.open` フラグで表示を制御）
- `on_enter_ozmux` 内で `Res<OzmuxConfigsResource>` から `startup_mode` を直接読んで分岐：
  - **`StartupMode::Ozmux`** → `SessionPicker.open = true` でピッカー表示。ユーザーが選択後に `handle_picker_input` が `attach_or_create` を呼ぶ
  - **`StartupMode::AutoAttach`** → `select_attach_target(server.list_sessions())` で最善セッションを選び直接 `attach_or_create` を呼ぶ

---

## Section 5: デタッチショートカット

### `crates/configs/src/shortcuts.rs` への追加

```rust
// Bindings 構造体に追加
pub detach_session: Option<KeyChord>,  // デフォルト: 未設定

// ShortcutAction enum に追加
DetachSession,
```

処理は `src/input/shortcuts.rs` の既存ディスパッチパスに `ShortcutAction::DetachSession` ブランチとして追加する。独立した keyboard reader システムは追加しない。

### 処理系（`src/input/shortcuts.rs` の既存ディスパッチに追加）

```rust
// 既存の shortcut dispatch ブランチに追加
ShortcutAction::DetachSession => {
    next_state.set(AppMode::Ozma);
    // OnExit(AppMode::Ozmux) が自動的に切断・クリーンアップを担う
}
```

`.run_if(in_state(AppMode::Ozmux))` でゲートする（既存ディスパッチシステムごとゲートするか、ブランチ内でガードする）。

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
| `src/input/shortcuts.rs` | `ShortcutAction::DetachSession` バリアント追加、ディスパッチブランチ追加 |
| `Cargo.toml` (root) | `ozma_mode = { path = "crates/ozma_mode" }` 追加 |
| `crates/tmux_session/src/events.rs` | `TmuxConnectionReset` を `pub` に昇格し `lib.rs` から再エクスポート |
| `crates/ozma_mode/src/lib.rs` (test) | `OzmaModePlugin::new(None, AppMode::Ozma)` に更新 |
