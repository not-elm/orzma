# Phase 1: 単一ホストプロセス + ユーザー拡張可能なホストAPI — 設計

- Status: Draft (brainstorming approved; revised per `/forte:spec-review` 2026-06-11, pre-implementation)
- Date: 2026-06-11
- Scope: `docs/memo.md` の **Phase 1 のみ**。Phase 2 (OSC インライン Webview レンダリング) / Phase 3 (tmux -CC) は本書末尾にロードマップとして記録するのみで、本設計の対象外。

> **配置上の注意:** `docs/` はリポジトリで gitignore 済み(CLAUDE.md「Other notable paths」)。本 spec を `docs/` 配下に置くと **バージョン管理に入らない**(`git check-ignore` で確認済み)。PR でレビューしたい場合は `docs/` の外(例: トップレベル `specs/`)へ移すか、PR 説明に転記すること。

## 1. 背景

現状(commit `b431911` 時点)、ozmux は **拡張ごとに 1 つの Node.js プロセス**を起動する。`ExtensionControlPlugin` が `node bootstrap.ts` を spawn し、拡張ごとに 3 つの Unix ソケット(control NDJSON / handlers NDJSON / asset length-prefixed)と stdin コマンドシムを張る。`@memo` コマンド、`bootstrap()`、`handlers`/`channels` の RPC dict が、memo が「廃止」と呼ぶ Bootstrap / 拡張コマンド / Handler に相当する。

b431911 は OSC 5379 `mount;<view_id>` / `unmount` パーサ、`ViewRegistry`(拡張が control プレーンで view を事前登録し、PTY バイトは id 参照しかできない「gated, extension-registered」信頼モデル)、mount observer を既に追加済み。本 Phase はこの OSC mount 資産を**転用**する。

**spec-review で確認済みの前提(コード根拠あり):**
- OSC mount された Extension 面は、今日すでに **実体の CEF webview を spawn する**。`src/osc_webview.rs` が `SurfaceKind::Extension` + `OwningExtension` を立て、`src/ui/surface.rs` が `ExtensionSurfaceMarker` を付与、`src/extension_render.rs` の `finish_extension_setup` が `WebviewSource` + `PreloadScripts` を挿入する。
- webview の信頼された発信元は **`Receive<_>.webview: Entity`**(bevy_cef が per-webview client handler で束縛、JS payload 由来ではない)。現行 `on_ozmux_frame` も `frame.webview` を信頼経路に使っている。

## 2. ゴール / 非ゴール

### ゴール
- 拡張ごとのプロセスを廃止し、アプリ上に **Node.js プロセスをちょうど 1 つ**起動する。
- Bootstrap / 拡張コマンド / Handler(RPC dict + channels)を廃止する。
- エンドユーザーが **ホスト API を拡張**でき、Webview から `window.<namespace>.<method>(...args)` で呼べる。
- OS リソースアクセス(例: `window.fs.read(path)`)を **namespace 粒度の最小権限**で Webview に与える(下記「最小権限の粒度」参照)。

### 非ゴール (Phase 1 では扱わない)
- OSC インライン Webview レンダリング(Kitty 風テクスチャ埋め込み) — Phase 2。
- tmux -CC サポート — Phase 3。
- ストリーミング / subscribe / イベント(request/response のみ)。
- host プロセスの自動再起動。
- **method 粒度の capability**(Phase 1 は namespace 粒度)。
- `window.<ns>` 型の完全 codegen(将来課題。本 Phase は `.d.ts` augment で足りる)。

### 最小権限の粒度
capability は **namespace 単位**。`capabilities = ["fs"]` は `window.fs.*` の **全メソッド**を許可する(`read` だけではない)。拡張作者は 1 namespace に危険メソッドと安全メソッドを混在させないこと。method 粒度の grant(例 `"fs.read"`)は将来課題(§5)。

## 3. 決定サマリ

| # | 決定 |
|---|------|
| ① | 単一 `node` host プロセスを起動・監視。**既存 `ExtensionManagerPlugin`/`extension_manager` を単一ホストマネージャへ作り替える**(新規並列プラグインは作らない)。自動再起動なし、host 不在時はグレースフル reject |
| ② | `extensions/<name>/{api.ts, ozmux.toml, assets}`。`export default {...}`(任意で `defineApi(...)`)、namespace はグローバル一意・衝突は先勝ち+警告。capability は Rust 所有・namespace 粒度 |
| ③ | Approach A: Proxy 注入 → `cef.emit(<単一オブジェクト>)` → **Rust が webview `Entity` 起点で capability 検査** → host ソケット → `api[ns][method]`。結果は `reqId→Entity` 相関で返す。バイナリは base64 ラッパー(境界タグ) |
| ④ | 制御プレーン / Handler / コマンド / bootstrap を全廃。OSC mount・`ViewRegistry`・`ozmux-ext://` scheme・`EndpointRegistry`・`JsEmitEvent`/`HostEmitEvent` は転用。`extensions/*` を追加ルートとして導入 |
| ⑤ | capability 強制・manifest パース・バイナリ codec 往復・asset ルーティング・type-stripping ロード・E2E(memo)。`--test-threads=1` |

### トランスポート選定(Approach A)
- **A (採用):** 単一 RPC ソケット + 既存 CEF ブリッジ + Rust ルーター。capability 検査を Rust 1 点に集約。CLAUDE.md の「no daemon / no HTTP server」原則に合致。
- **B (却下):** host が localhost HTTP/WS を立て Webview が直接 fetch。「no HTTP server」原則に反し、面ごとの capability 強制が困難。
- **C (却下):** Node/V8 を Rust に埋め込み(napi / deno_core)。ビルド負荷が甚大で「ただ 1 つの Node プロセス」要件にも素直に合わない。

## 4. アーキテクチャ

### ① プロセス & ライフサイクル

**既存の `ExtensionManagerPlugin` / `src/extension_manager.rs` を「単一ホストマネージャ」へ作り替える**(新規 `HostProcessPlugin` を並列に足すのではなく、既存が持つ `EndpointRegistry` 共有・readiness 公開・ドレインを再利用する)。起動時に **`node <bundled-host-entry>` を 1 つだけ** spawn・監視する。

- **host runtime はアプリ同梱**(ユーザー提供ではない)。esbuild が `host/` パッケージを `assets/host.mjs` にバンドルし、Rust バイナリへ `include_str!` で埋め込む。実行時にランタイムディレクトリへ `host.mjs` として書き出し、`node host.mjs` として spawn する。host は `<repo>/extensions/*` と `~/.config/ozmux/extensions/*` の `api.ts` を **dynamic import** して API オブジェクトを集約し、アセット配信と RPC ディスパッチを担う。
- **spawn 時の env:** 単一 RPC ソケットパス・アセットソケットパス・extensions ディレクトリパス。ランタイムルート(ソケット置き場)は **1 つだけ**(0700 perms)。現状の per-PID / per-extension ディレクトリツリーは廃止。
- **readiness:** host が全拡張のロード後に `.ready` を返す。ロード失敗(後述の type-stripping 制約違反を含む)は名前付きで報告 → Rust 側でログ。Rust はタイムアウト付きで待機。
- **監視 / 障害:** Phase 1 は **自動再起動なし(YAGNI)**。host がクラッシュ / exit したら `HostProcessDown` 状態を立て、以降の host-API 呼び出しは `host_unavailable` で **グレースフルに reject**。自動再起動は将来課題。
- **終了:** アプリ終了時に子プロセスへ SIGTERM、ランタイムルートを掃除。

**Node ネイティブ TS type-stripping の制約(設計拘束):** host loader は `import('/abs/path/api.ts')` で読む。Node のネイティブ stripping は **(a) erasable な TS 構文のみ**(`enum` / parameter properties / `namespace` 不可、違反は `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`)、**(b) dynamic import 指定子に `.ts` 拡張子必須**、**(c) `tsconfig` を無視**(`paths` 不可)、**(d) `node_modules` 配下の TS は stripping 対象外**。拡張 `api.ts` は erasable TS に限定する。将来フルトランスパイルが要る場合は `tsx` / 同梱バンドル host へ切替(§5)。

**置き換える対象:** `ExtensionControlPlugin` / `CommandExtensionConfig` / `CommandExtension`(per-extension spawn)、`extension_manager` の「拡張ごとに discover→spawn」ループ、per-extension `RuntimeRoot` ツリー。

### ② 拡張構成 & manifest

拡張は npm パッケージではない(SDK `bootstrap()` 依存が廃止されるため)。1 拡張 = 3 要素:

```
~/.config/ozmux/extensions/<name>/
├── api.ts          # export default { fs: { read(path) {...} } }
├── ozmux.toml      # views と必要 capability の宣言(Rust が読む信頼データ)
└── <assets>        # index.html 等。ozmux-ext://<extension>/<entry> で配信
```

**`api.ts`(host が読む / コード)**
```ts
// 最小形(SDK import 不要・標準)
export default {
  fs: { read: async (path: string) => await readFile(path) },
};

// 型推論を効かせたい場合(任意・@ozmux/sdk を解決できる拡張のみ)
import { defineApi } from "@ozmux/sdk/extension";
export default defineApi({ fs: { read: async (p: string) => await readFile(p) } });
```
- **host loader は `(await import(p)).default` のトップレベルキーを namespace として集約**する。loader はプレーンオブジェクトをそのまま受ける(`defineApi` は実体ゼロコストの恒等関数で、付けても付けなくても同じ)。
- **`defineApi` は任意の型推論用シュガー**。SDK が `export function defineApi<const T extends ApiNamespaceMap>(api: T): T { return api }` を提供し、将来 `typeof` ベースの `window.<ns>` 型 codegen の布石にする。**ただし `@ozmux/sdk/extension` を import 解決できるのは workspace 内拡張のみ**(下記モジュール解決の論点)。スタンドアロンなユーザー拡張は import を省いてプレーン default export を書く。
- **default export のトップレベルキーが namespace**(memo の `fs` がそのまま namespace)。1 拡張が複数 namespace を提供可。
- **namespace はグローバルに一意**。複数拡張で衝突したら、ロード順(拡張ディレクトリ名のソート順)で **先勝ち**、後発の衝突 namespace は **スキップ+警告ログ**(fail-soft)。

> **オープン論点(モジュール解決):** スタンドアロンな `~/.config/ozmux/extensions/<name>/api.ts` は npm パッケージではないため `@ozmux/sdk/extension` を自動解決できない。かつ Node は `node_modules` 配下の TS を stripping しない。Phase 1 の既定は「ユーザー拡張は SDK を import しないプレーン default export」。SDK を使いたい拡張向けの解決策(import map / `NODE_PATH` / workspace 限定 / `tsx`)は §5 で決める。

**`ozmux.toml`(Rust が読む / 信頼データ)**
```toml
[[views]]
id = "memo.main"          # OSC mount が参照する view_id
entry = "index.html"      # ozmux-ext://memo/index.html
capabilities = ["fs"]     # この view の webview に注入を許す namespace 群(namespace 粒度)
interactive = true
```

**セキュリティ上の肝(コードと信頼データの分離):**
- **capability(信頼データ)は Rust が `ozmux.toml` から直接パース**して `ViewRegistry` に載せる。Node が報告した値は信用しない。「どの面がどの namespace を呼べるか」は **Rust 所有**となり、任意ユーザーコードを実行する host プロセスが capability を詐称できない。
- view が要求した capability に対応する namespace をどの拡張も提供しない場合 → ロード警告。

**スキャン対象:** ユーザー(`~/.config/ozmux/extensions/`)は常時、同梱(`<repo>/extensions/`)は `#[cfg(feature = "debug")]` の下でのみ。常時有効なルートは `~/.config/ozmux/extensions`、プロジェクトルートの `extensions/` は `debug` cargo feature 限定。名前重複は **ユーザー優先**(上書き可能)。
> **注意(順序の変更):** 現行 discovery は **bundled 先・first-wins**(`extension_manager.rs:98,145` で bundled を先に push し重複は初出を採用)。ユーザー優先にするには **ユーザールートを先にロード**するか上書きロジックを明示的に入れる必要がある。これは現行挙動の意図的な反転であり、実装時に明記する。

### ③ ホスト API ブリッジ(RPC / capability / シリアライズ)

Approach A の心臓部。**既存の `JsEmitEventPlugin` / `HostEmitEvent` / `PreloadScripts` 機構をそのまま再利用**し、変えるのは「Node 側ターゲット(単一ソケット + capability 関所)」と「注入する JS(`window.ozmux.call` → namespace Proxy)」だけにする。

**注入(Rust → webview)**
- Extension 面の webview **生成時**(`WebviewSource` 挿入と同時、ページスクリプトより前)に、`PreloadScripts` として **Proxy ブリッジを注入**(現行 `ozmux.js` を置換)。その面の許可 namespace は、mount 時に Rust が `ViewRegistry` の caps をコピーして surface entity に立てる **`GrantedNamespaces` コンポーネント**から取る。許可 namespace 分だけ Proxy を生成:
  ```js
  window.fs = new Proxy({}, { get: (_, m) => (...args) => __hostCall("fs", m, args) });
  ```
- 許可されていない namespace は `window` に存在しない(`undefined`)。Browser 面には一切注入しない。
- **NOTE(load-bearing):** ブリッジは **`PreloadScript` でなければならず、グローバル `CefExtension` にしてはならない**。context 生成時に binding を呼ぶグローバル拡張は render プロセスをクラッシュさせる(`src/extension_render.rs` の既存 NOTE)。

**呼び出し経路(webview → Rust → host)**
1. `window.fs.read(p)` → `__hostCall` が `reqId` を採番し Promise を保留 → **`cef.emit({reqId, ns, method, args})`**。
   - **注意:** bevy_cef の binding は `arguments.first()` だけを直列化する(`cef_api_handler.rs`)。`cef.emit(eventName, payload)` の 2 引数形は payload を**捨てる**ため使わない。固定の `ozmux` チャネル上に **単一オブジェクト**を載せる。
2. **Rust が capability を検査(信頼の関所)**: 信頼される発信元は **`Receive<_>.webview: Entity`**(per-webview client handler で束縛、JS payload 由来ではない)。その entity の **`GrantedNamespaces`** を読み、`ns ∉ caps` なら即 `capability_denied` で reject、**host へは転送しない**。文字列の "surfaceId" を信頼鍵にしない。
3. 検査通過 → 単一 host ソケットへ **`{reqId, ns, method, args}`** を framed 送信(`reqId` は相関用、信頼鍵ではない)。
4. host が `api[ns][method](...args)` を実行。Rust が信頼の関所なので host 側は再検査しない(単純さ優先)。

**結果(host → Rust → webview)**
- host は **`{reqId, ok: true, value}` / `{reqId, ok: false, error}`** を返す(discriminated union; これが実装済み `dispatch.ts` の `HostResultFrame` の正準形)。Rust は **`reqId → webview Entity` の in-flight 相関**から発信元 entity を引き、**`HostEmitEvent::new(webview, ...)`**(既存 outbound `ozmux` チャネル)で**その webview にだけ**返す。Proxy が Promise を resolve/reject。

**シリアライズ**
- 既存 CEF ブリッジは **JSON 文字列チャネル**(`HostEmitEvent` は文字列を配送)。プレーン値は JSON でそのまま。
- バイナリ(`fs.read` の `Buffer`/`Uint8Array`)は **`{ __u8: "<base64>" }` ラッパーに符号化**。**境界タグ方式**:host が明示的に返したトップレベルの `Buffer`/`Uint8Array` をラップする(任意のネスト結果を再帰ディープウォークしない — CPU 税と `__u8` キー衝突を避ける)。webview 側 Proxy は境界でデコード。引数経路も対称。
- **ガードレール:** `fs.read` 等で巨大ファイルが単一 JSON 文字列として render プロセスを跨ぐと詰まる。**最大レスポンスサイズ上限**を設け、超過はエラーにする(閾値は実装時に決定)。base64 は +33% のオーバーヘッドを許容(Phase 1)。専用バイナリチャネル / MessagePack は将来(§5)。

**エラー伝播**
- host method の throw → `{err, message}` → webview の Promise が `Error` で reject。
- `capability_denied` / 未知 ns・method / `host_unavailable`(① のクラッシュ時)も構造化 reject。

**型付け**
- ランタイムは Proxy(動的)。Phase 1 は **拡張が `.d.ts` で `Window` を augment** する軽量方式で足りる(完全な codegen は将来課題。手書き augment は `api.ts` 実体と乖離し得る点に留意)。

### ④ 撤去対象 & 移行

**撤去するもの**
- `ExtensionControlPlugin` / `CommandExtensionConfig` / `CommandExtension`。
- `extension_manager` の「拡張ごとに discover→spawn」ループ(→ 単一 host spawn + Rust の manifest スキャンに置換)。
- コマンドシム:`@memo` シェルコマンド、`bin_dir` の shim、stdin コマンドフレーム。
- Handler モデル:`handlers` dict・`channels`・`bootstrap()` SDK エントリ・handlers ソケット・`handlers_bridge.rs`。
- 制御プレーン(ソケット):`register_view`/`split`/`add_surface`/`activate` の op、`control.rs` の parse、`control-client.ts`、`pane.ts`(`ctx.pane.split`)。プログラム的レイアウト操作は廃止、レイアウトは in-app のまま。
- `window.ozmux.call/subscribe`(Handler RPC)→ `window.<ns>.<method>` に置換。

**残す / 転用するもの**
- OSC webview パーサ(`osc_webview.rs`)、`OscMounted`/`NonInteractive`、mount/unmount observer。
- `ViewRegistry` ——ただし **manifest 由来でロード**するよう変更し、**`capabilities` フィールドを追加**。mount 時に caps を `GrantedNamespaces` として surface entity へコピー。
- `JsEmitEventPlugin` / `HostEmitEvent` / `PreloadScripts` 機構(③ で再利用)。
- `EndpointRegistry` ——**そのまま残し、全拡張名を「単一 host のアセットソケット」1 つへ向ける**(name→endpoint マップは不変、scheme handler は無改修)。
- `ozmux-ext://` scheme + アセット配信。bevy_cef 連携、`extension_render` の surface 描画。

> **アセットプロトコルの変更(必須):** `scheme.rs` は `ozmux-ext://<name>/<path>` を `<name>` で dispatch するが、現行の `fetch(&endpoints, path)` / `protocol::Request { path }` は **`<name>` を落として `path` だけ**を host へ渡す(`scheme.rs:75`, `protocol.rs:13`)。全名が同一ソケットを指す単一 host 構成では、host が**どの拡張か判別できない**。アセットリクエストを **`{ extension, path }`**(または `extension/path` プレフィックス)へ拡張する。Node 側 `serveAssets` と protocol fixture も更新。

**移行(example / test fixture)**
- 現 `@memo` を **新モデルの同梱サンプル拡張**に作り替え:`extensions/memo/{api.ts(例: `fs` namespace), index.html(`window.fs.read` を呼ぶ), ozmux.toml(view `memo.main`, capabilities=["fs"])}`。正典の例 + 統合テスト fixture とする。
- **`extensions/*` を追加ルートとして先に導入**(host-API モデルに必要なのはこれだけ)。

**SDK 変更**
- `@ozmux/sdk`:`./server`(bootstrap/control/handlers)・`./cmd-shim` を **削除**。
- **`@ozmux/host` を新設**(private パッケージ) ——同梱 host ランタイム(extension loader=api.ts の dynamic import、アセットサーバ、単一ソケット上の RPC ディスパッチ)。esbuild で `assets/host.mjs` にバンドルし(`pnpm -C host build`)、Rust へ `include_str!` で埋め込む。`node` が実行するのはこれ。`defineApi` の作者向け面は `@ozmux/sdk`(`sdk/typescript/src/extension/`)に残す。
- `./surface`:`window.ozmux` 型を host-API クライアント型 + バイナリ codec + `Window` augmentation ヘルパーに置換。

### ⑤ テスト戦略

**Rust ユニット / 統合(`cargo test`)**
- **capability 強制(最重要):** granted に含まれない namespace を呼ぶと `capability_denied` で reject され host へ転送されないこと。**信頼鍵が `Receive<_>.webview` Entity 由来**で、JS payload の "surfaceId" を詐称しても勝てないこと。`GrantedNamespaces` コンポーネント経由の O(1) チェック。
- **manifest パース:** `ozmux.toml` → `ViewRegistry`(views + capabilities)、namespace 衝突の先勝ち+警告。
- **OSC mount リンク:** mount で `GrantedNamespaces` が surface entity に立つ、unmount(despawn)で自動解放。
- **アセットルーティング:** `{extension, path}` 拡張リクエストが正しい拡張のアセットへ解決されること。
- **host ライフサイクル:** `host_unavailable` 時のグレースフル reject。

**host ランタイム(vitest, `@ozmux/host`)**
- extension loader が複数 `api.ts`(プレーン default export と `defineApi` 形の両方)の namespace を集約、重複 namespace を先勝ちで解決。
- erasable TS の `api.ts` がロードでき、非 erasable 構文は名前付きエラーで報告されること。
- RPC ディスパッチが正しい `api[ns][method]` を呼ぶ。未知 ns/method はエラー。
- **バイナリ codec の往復:** 境界タグの `Uint8Array` → `{__u8}` → `Uint8Array`(引数・結果の両経路)。最大サイズ超過がエラーになること。
- host method の throw がエラーフレームになる。

**統合(E2E)**
- 同梱 `memo` 拡張で host プロセスを起動 → OSC mount → webview が `window.fs.read` を呼び期待バイト列を取得 → 未許可 namespace は reject。既存の `bevy_terminal/tests` / `extension_render` ハーネスを再利用。

**既知の注意:** 既存の IME テスト failure + 並列 teardown SIGSEGV があるため、グリーン確認は `--test-threads=1` + 該当 `--skip` を用いる。

## 5. オープンな論点 / 将来課題

- **SDK モジュール解決(要決定):** スタンドアロンなユーザー拡張が `@ozmux/sdk/extension`(`defineApi` 等)を使う手段 — import map / `NODE_PATH` / workspace 限定 / `tsx` 同梱ローダのいずれか。
- host プロセスの自動再起動(backoff 付き)。
- 大きいバイナリ向けの専用バイナリチャネル / **MessagePack**(JS 側 `@msgpack/msgpack`、Rust 側 `rmp-serde`)。ただし現行 IPC は文字列配送のため、bevy_cef チャネルがバイト搬送に拡張されて初めて base64 税が消える(Phase 2+)。
- `fs.read` 等の最大レスポンスサイズ閾値の確定。
- **method 粒度の capability**(例 `"fs.read"`)と、`zod`(catalog 既存)による **host 側引数バリデーション**(capability 関所の後、`api[ns][method]` の前)。
- `defineApi` の `typeof` からの `window.<ns>` 型の完全 codegen(手書き `.d.ts` augment の乖離を解消)。

## 6. ロードマップ(本設計の対象外 / 記録のみ)

- **Phase 2 — OSC インライン Webview レンダリング:** Kitty 風にターミナルグリッドへ Webview テクスチャを埋め込む。alt-screen(1049h で mount / 1049l で auto-unmount)に紐付け。Phase 1 の host API・OSC mount・ViewRegistry をそのまま土台にする。
- **Phase 3 — tmux -CC サポート:** 通常は単一エミュレータとして起動し、ショートカットで tmux セッション一覧 → 指定セッションを `-CC` 制御モードでアタッチ。in-app レイアウト管理を tmux 制御へ置換。Webview とは独立。
