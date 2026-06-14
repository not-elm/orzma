# `extension` → `webview` 命名統一 & 残骸削除 — Design

- Date: 2026-06-14
- Status: Approved (pending implementation plan)
- Branch: `remove-ext`

## 背景

`crates/extension_host` をはじめ `extension` を冠した名前は、かつて存在した
Extension 機能に由来する。その機能はすでに削除され（`04405cb` で Extension
apparatus 除去、`dbb4f6d` で dormant NodeJS host-RPC 除去）、現在は webview の
アセット配信と IPC を管理する仕組みに置き換わっている。名前と実体が乖離して
いるため、`webview` 命名に統一する。

## 方針

役割別 `webview_*` への機械的リネームを基本とし、削除済み Extension 機能の
残骸（生きた呼び出し元がないコード）は同時に除去する。意味的に「ファイル
拡張子」を指す `extension` はリネーム対象外。

決定事項:

- 命名は役割別 `webview_*`（単一の `webview_ipc` には寄せない）。クレートは
  アセット配信 + ランタイムソケットを担い、render モジュールは描画を担う、と
  役割が分かれているため。
- 削除済み機能の残骸は削除する（リネームして残さない）。
- JS コンテキストの `extensionName` フィールドおよび Rust 側 `extension_name`
  引数は完全削除する（常に空文字・リポジトリ内外問わず参照者なし）。
- クレートの公開 API 名（`DynAsset`, `DynAssetRegistry`, `custom_dyn_scheme`,
  `RuntimeRoot`）は据え置く。いずれも webview ホスティングの役割に合致しており
  `extension` 語を含まないため。
- 歴史的設計文書（`docs/superpowers/plans|specs/` の既存ファイル）は改変しない。
  当時の記録としての価値を優先する。

## 変更詳細

### 1. クレートのリネーム

`crates/extension_host/` → `crates/webview_host/`

- パッケージ名 `ozmux_extension_host` → `ozmux_webview_host`
  (`crates/webview_host/Cargo.toml`)
- `lib.rs` / `host.rs` / `asset.rs` 内のコメントで Extension 機能由来の
  「extension sockets / names / dir / directory / ships」を `webview` に置換。
- 公開 API のシンボル名は変更しない。

### 2. バイナリモジュールのリネーム

`src/extension_render.rs` + `src/extension_render/`
→ `src/webview_render.rs` + `src/webview_render/`

- プラグイン `OzmuxExtensionRenderPlugin` → `OzmuxWebviewRenderPlugin`
- 配下ファイル移動: `preload.rs`, `ozmux_bridge.js`
- 参照更新: `src/main.rs`（`mod` 宣言 / `use` / プラグイン登録）,
  `src/inline_webview.rs`, `src/control_plane.rs`

### 3. 残骸の削除

生きた呼び出し元が存在しない（定義／自テストのみ）ことを確認済み:

- `crates/configs/src/path.rs`: `extensions_dir()` と `EXTENSIONS_REL_PATH`
  (`"ozmux/extensions"`)、および関連テスト 4 件
  (`extensions_dir_uses_xdg_when_set`,
  `extensions_dir_falls_back_to_home_config`,
  `extensions_dir_ignores_ozmux_config_var`,
  `extensions_dir_errors_when_no_xdg_and_no_home`)
- `crates/configs/src/shortcuts.rs`: `NewExtensionSurface` variant
- `src/webview_render/preload.rs`: `context_preload_js_role` の
  `extension_name` 引数と、フォーマット文字列中の `extensionName:{n:?}`
  （`__ozmuxContext.extensionName`）を削除。`build_dynamic_preload` 側の
  呼び出しも合わせて更新。

### 4. クロス参照・依存の更新

- root `Cargo.toml`: 依存行（パッケージ名 + path）を `ozmux_webview_host` /
  `crates/webview_host` に更新。L17 付近の「extension webview」コメントも更新。
- `use ozmux_extension_host::…` を含む 3 ファイル: `src/control_plane.rs`,
  `src/main.rs`, `src/webview_render.rs`。
- コメント「extension surface / pane」: `src/input.rs`, `src/clipboard.rs`,
  `src/input/mouse_buttons.rs`。

### 5. ドキュメント

- `CLAUDE.md` を更新（モジュールマップ・クレート説明・プラグイン名:
  該当箇所 L14, L18, L22, L41 相当）。
- `docs/superpowers/plans|specs/` の既存ファイルは非改変。

## 対象外（ファイル拡張子の意味、保持）

`crates/extension_host/src/asset.rs`（移動後 `crates/webview_host/src/asset.rs`）
の MIME 判定まわりは「ファイル拡張子」を指すため保持する:

- `path.extension()` 呼び出し
- doc: "Maps a file extension to a bare MIME type …"
- "Unknown extensions fall back to `application/octet-stream`"
- テスト `mime_for_common_extensions`

ただし同ファイル内でも「extension directory」「extension ships」のような
削除済み機能由来のコメントは `webview` に置換する（文脈で判別）。

`crates/ozma_tty_renderer/src/material.rs` の "wire extension" も無関係のため
保持する。

## 検証

1. `cargo build`（ワークスペース全体、`extension_host` の `cef` feature 含む）
2. `cargo test`
3. `cargo clippy --workspace`
4. `cargo fmt`
5. `pnpm -r test`（SDK への影響はない想定だが確認）

リネーム漏れの最終チェックとして、`grep -rin "extension" --include="*.rs"
--include="*.toml"` の結果がファイル拡張子由来のもの（`asset.rs` の MIME、
`material.rs` の wire extension）のみになっていることを確認する。

## 非目標

- 公開 API のシンボル設計変更（`DynAsset` 等のリネーム）。
- webview / IPC の機能変更・リファクタ。
- 歴史的設計文書の書き換え。
