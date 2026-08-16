# orzma_tty_engine 置き換えに向けた機能差分調査

調査日: 2026-08-16

## 1. 目的と前提

`orzma_tty_engine` を新スタック(`bevy_orzma_term` + `orzma_term` + `orzma_vt`)で完全に置き換えられる状態かを調査した。

前提は次のとおり。

- `orzma_vt` 側の既知の未実装(`OrzmaVt::frame()` の `todo!()`、`history_size()`、`drain_replies_into()` のバックエンド実装、`drain_signals()` の空イテレータ、hyperlink ID 置換。詳細は [orzma_vt_backend_responsibility.md](orzma_vt_backend_responsibility.md) §12)は評価から除外する。
- API 設計が変わっているため上層クレートに改修が必要になることは織り込み済みとし、**API 形状の差ではなく機能の不足**を対象とする。

調査方法: 旧エンジン全 21 ソースファイルの精査、上層クレート(root バイナリ `src/`、`orzma_tty_renderer`、`orzma_webview`)の実利用箇所の洗い出し、新スタック全ファイルの精読を突き合わせた。

## 2. 結論

**現時点では完全差し替えできる状態ではない。**

シグナル語彙・キー/マウスエンコーダ・選択コア・スクロール・PTY spawn は旧エンジンとほぼ等価(改善されている箇所も多い)だが、次の 3 系統の穴がある。

1. **駆動系の未結線** — PTY 出力からフレームが一切生成されない。ChildExit・DSR/DA 応答も未配線。
2. **マウスルーティング層の丸ごと欠落** — 旧 `buttons.rs` / `wheel.rs` の純ルーターに対応物がない。
3. **webview アンカー機構の未移植** — OSC 5379 の anchor 刻印・`history_base` 追跡・飽和ゲートに設計上の置き場所すらない。

推奨着手順は §8 に示す。

## 3. ブロッカー級の機能不足(orzma_term / bevy_orzma_term)

### 3.1 PTY 出力でフレームが出ない

[`OrzmaTerm::pump`](../crates/orzma_term/src/lib.rs) は `interpret` 後に coalescer を arm しない。`Coalescer::is_due` は disarm 状態で常に偽なので、emit の契機が scroll / resize / selection しかなく、**シェルが何を出力しても描画されない**。

付随して次も未結線である。

- `Coalescer::should_flush_immediately`(bootstrap 即時 emit、`pending_user_input` による入力エコーの低遅延化)は [coalescer.rs](../crates/orzma_term/src/coalescer.rs) に移植済みだが、呼び出し側が存在しない。
- `pending_user_input` を立てる仕組み自体がない(旧エンジンは `TerminalHandle::write` が書き込み**前**にフラグを立てていた。この順序は load-bearing としてドキュメント化されていた)。
- 旧 `flush_due_terminals` の bootstrap 救済(`needs_bootstrap_emit` → `force_bootstrap_damage`)に相当する初回スナップショット保証がない。

`pump` 内の `TODO: DamageVerdictを使い、coalescerのdeadlineを調整する` がこの箇所を指す。

### 3.2 ChildExit が発火しない

[`Pty`](../crates/orzma_term/src/pty.rs) は `exit_rx` を保持するがアクセサがなく、`pump` も読まない(TODO 記載)。`TermSignal::ChildExit` は定義のみで生成経路がなく、bevy 層の `TermChildExitSignal` は永遠に発火しない。

ホストは `TerminalChildExit` を受けてアプリを終了しており(`src/session/exit.rs`)、これが鳴らないとシェル終了後もウィンドウが残る。

### 3.3 DSR/DA 応答の PTY 書き戻し経路がない

旧エンジンは `drain_pty_writes`(`crates/orzma_tty_engine/src/lib.rs`)で alacritty の `Event::PtyWrite`(DSR カーソル位置報告、DA 等)を毎フレーム 1 回の `write_all` にまとめて PTY へ書き戻していた。

新スタックは `OrzmaVt::drain_replies_into` の口はあるが、`pump` がそれを呼ばず PTY へも書かない。バックエンド側の `todo!()`(orzma_vt 側、対象外)とは**独立に、orzma_term の配管が欠落**している。DSR に答えない端末では vim 等のフルスクリーンアプリが誤動作・ハングする。

### 3.4 ホイールルーティングが未実装

[`orzma_term/src/input/wheel.rs`](../crates/orzma_term/src/input/wheel.rs) は全 201 行がコメントアウトされている。旧 [`WheelAction::route` / `route_horizontal`](../crates/orzma_tty_engine/src/wheel.rs) が持っていた三分岐ロジックの対応物がない。

| 優先順 | 条件                                           | 旧挙動                                                                                                                          |
| ------ | ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| 1      | mouse tracking 有効                            | ノッチ数(上限 `max_protocol_events_per_frame = 8`)だけ 64/65(横は 66/67)の報告を連結して PTY へ。`lines_per_notch` は乗算しない |
| 2      | `ALT_SCREEN` + `ALTERNATE_SCROLL`(DECSET 1007) | ノッチ × 行数(通常 3 / fine 1)回の **SS3 矢印**(`APP_CURSOR` 非依存、Shift バイパスなし)                                        |
| 3      | それ以外                                       | ビューポートスクロール(`ScrollViewport`)                                                                                        |

`WheelModifiers.alt` → プロトコルの meta ビットへ寄せるマッピング、横スクロールは mouse-mode 専用(スクロールバック変換なし)という決定も含めて未移植。ホスト `src/input/mouse/wheel.rs` はこのルーターに全面依存している。

### 3.5 ボタンルーティングが未実装

旧 [`ButtonAction::route`](../crates/orzma_tty_engine/src/buttons.rs) の判断層がどこにもない。新スタックにあるのは `MouseReport` エンコーダ(送信の最終段)のみ。

未移植の決定事項:

- アプリキャプチャ(`MOUSE_MODE`)vs ローカル選択の振り分けと **Shift バイパス**
- **Alt + 左プレス → 即 Block 選択**(クリック数不問)
- クリック数 1 / 2 / 3 → `Simple`(遅延 ArmDrag)/ `Semantic` / `Lines`
- プレス転送時の `ClearAndWriteToPty`(古いローカルハイライトの破棄)
- 転送イベントの毎フレーム上限(`ButtonConfig::max_protocol_events_per_frame`)

### 3.6 vi mode / vi motion が空スタブ

`bevy_orzma_term/src/requests/vi_mode.rs` の `apply_vi_mode` と `vi_motion.rs` の `apply_vi_motion` は**本体が空**。

- `OrzmaVt::switch_vi_mode` は実装済みなのに未接続。
- vi motion は `VtBackend` にメソッドが存在せず、[`ViMotion`](../crates/bevy_orzma_term/src/requests/vi_motion.rs) は bevy 層の仮置き enum(doc コメント自身が「最終的な置き場所は VT 層」と明記)。
- 旧エンジンの付随挙動も未移植: 相対スクロール時の vi カーソル画面行追従(`track_vi_cursor_after_scroll`)、`scroll_to_top/bottom` での vi カーソル移動、vi 終了時の live-tail スナップ。

### 3.7 ホスト向け状態アクセサの欠落

`OrzmaTermHandle`(Deref 先 `OrzmaTerm`)が再公開していない読み取り面。上層の現行機能が直接依存しているものに限る。

| 欠落アクセサ                      | 旧 API                       | 上層の用途                                                                     |
| --------------------------------- | ---------------------------- | ------------------------------------------------------------------------------ |
| `modes()`                         | `current_modes()`            | マウスルーティング判断・Shift バイパス・focus 報告ゲート(`src/input/mouse.rs`) |
| `selected_text()`                 | `selection_to_string()`      | **コピー操作の本体**(`CopyAction`)                                             |
| `selection_kind()`                | `selection_type()`           | vi の v/V トグル判定                                                           |
| `display_offset` / `history_size` | `vi_indicator_snapshot()`    | vi インジケータの `[offset/total]` 表示                                        |
| 任意タイミングの強制 emit         | `flush_emit` + `_vt_only` 系 | PTY 非接続端末の適用経路(`src/action/terminal.rs`)                             |

また旧 `selection_change_type` の「アンカーが無ければ `false` を返し、呼び出し側が新規開始にフォールバックする」という戻り値契約が、新 `change_selection_kind`(戻り値は staged damage の有無)で失われている。

## 4. フレーム契約の機能的欠落(orzma_webview が壊れる系)

### 4.1 OSC 5379 アンカー機構が丸ごと未移植

旧 `handle.rs` の一大サブシステムに対応物がない。

- OSC **バイト位置**でのアンカー刻印(チャンク末尾ではない)と `frame_seq` の同時刻印(`force_next_emit`)
- `CSI ?2026` 同期更新を `stop_sync` でフラッシュしてからカーソルをサンプリング
- `history_base` の単調追跡と、`CSI 3 J` 等での履歴フォールド時の合成 unmount-all
- スクロールバック飽和(cap 10,000 行)時の primary-screen mount 拒否(alt-screen は免除)
- alt-screen では `FixedScreen`、primary では `Scrollback` アンカー

新スタックでは `TermApcWebviewSignal.anchor` のフィールドはあるが生成側が存在しない。さらに `FrameSnapshot` の `history_size` / `history_base` は「別のアプローチを考えたい」として[コメントアウト中](../crates/orzma_vt/src/schema/frame.rs)であり、`orzma_webview` のオーバーレイ投影(`viewport_row = line - (history_base + history_size - display_offset)`)と seq の回り込み比較(serial-number arithmetic)はこの契約に直接依存する。**「別アプローチ」の設計が決まるまで webview 統合は成立しない。**

### 4.2 スナップショットに `modes` がない

旧ワイヤは `modes: Vec<String>`(追跡 8 フラグ: `alt-screen`, `bracketed-paste`, `app-cursor-keys`, `focus-events`, `mouse-vt200`, `mouse-btn-event`, `mouse-any-event`, `mouse-sgr-1006`)をスナップショットで運び、`orzma_webview` は `TerminalGrid.modes` と `TerminalModeChanged.removed` の両方で `"alt-screen"` を照合している。

新 `FrameSnapshot` に modes はなく、`ModeChange` シグナル(差分)だけでは絶対状態の再同期ができない。「alt-screen 遷移は必ず Snapshot で届く(delta には載らない)」という旧不変条件の置き場所も未定。

### 4.3 `SelectionKind` に `Block` / `Semantic` がない

[`schema/selection.rs`](../crates/orzma_vt/src/schema/selection.rs) で両変性がコメントアウトされ、`From<SelectionType>` は `todo!("Not supported yet")`。ホストは現に Alt+ドラッグ(Block)、ダブルクリック(Semantic)、vi キーマップ(Block)で使用している。§3.5 のボタンルーターを移植すると同時に必要になる。

## 5. orzma_vt 側の未実装に「付随して」失われやすい仕様

評価対象外だが、`drain_signals` 実装時に一緒に運ばないと消える仕様として記録する。

- **タイトルのサニタイズ**(`title.rs`): C0 / C1 / bidi 制御 / ゼロ幅文字の除去、256 字上限 + `…`。ウィンドウタイトルへの制御文字注入対策。
- **OSC 7 の検証**(`osc7.rs`): `file://` 必須、ホスト名照合(空 / `localhost` / 自ホスト名)、`;` を含むパスの再結合、バイト単位パーセントデコード、プロンプト毎再送の重複排除。なお旧実装は `std::os::unix::ffi::OsStringExt` 使用で Unix 専用だった — 新設計では要対処。
- **OSC 5379 の検証限界**(`osc/webview.rs`): view_id 1..=128 字 `[A-Za-z0-9._-]`、rows 1..=200、cols 1..=400、C1 8-bit 導入子の拒否。`orzma_webview/src/webview/mount.rs` はこの境界をエンジン責務として明記している。
- **mode diff の追跡 8 フラグとワイヤ文字列**の互換(§4.2 の表)。
- **`ClipboardStore`**(OSC 52 write)のイベント化。

## 6. パリティ確認済み(不足ではない)

- **シグナル語彙は旧イベントと 1:1** — Bell / Title / ResetTitle / Clipboard / Cwd / ApcWebview / ModeChange / ChildExit。旧エンジンで上層未購読だった Bell / CurrentDir / ClipboardStore は元々ホスト側機能が未実装であり、差し替え障害ではない。
- **キーエンコード** — 同じ 14 キー語彙、Ctrl 文字 → C0、meta-sends-escape、DECCKM。新実装は Home/End にも DECCKM を適用(旧は固定 `CSI H/F`)する xterm 準拠方向の改善。F1-F12 / Insert / CSI-u / modifyOtherKeys 非対応は旧と同等。
- **ペーストは新スタックが上位互換** — 旧エンジンにはペースト API がなくホストが括っていたが、新 `send_paste` は括弧付け + 埋め込みマーカー除去(ペーストインジェクション対策)+ 改行正規化 + scroll-on-input を内蔵し、テストで固定済み。
- **マウスプロトコルエンコーダ** — SGR / X10、release センチネル、223 クランプ、alt/meta の単一 meta ビット合流、ホイールボタン 64..=67。UTF-8(1005)を X10 へフォールバックする決定も一致。
- **Coalescer** — IDLE 3ms / MAX_CAP 12ms / MANY_ROWS_INSTANT_CAP 4、`Full` を即時フラッシュ対象から除外する不変条件まで忠実移植(未結線は §3.1)。
- **リサイズ** — ゼロ軸 / 上限(4096)ガードと PTY-first の失敗原子性は新規追加の改善。
- **スクロール** — ページ量(全画面高)、クランプ時の no-op 判定、scroll-on-input ポリシー。
- **選択コア(Simple / Lines)** — アンカー保持の粒度切替、vi カーソル起点開始、スクロールバックへのドラッグ。
- **PTY spawn** — macOS ログインシェルラッパー(`/usr/bin/login` + `.hushlogin` 分岐)を忠実移植。detached モードは書き込みシンク注入式に改善。

## 7. 付記

- **テストがコンパイル不能** — `vt_mut()` が未定義のため `cargo check -p orzma_term -p bevy_orzma_term --tests` が E0599 ×9 で失敗する。テスト側が想定する `pub fn vt_mut(&mut self) -> &mut OrzmaVt<B>` の追加が必要。
- **移植不要と判断できる旧 API**(上層から未使用を確認済み): `vi_goto`、`has_visible_content`、`snap_to_bottom_vt_only`、`cursor_changed`、`pending_user_input` アクセサ、イベント `TerminalTitleChanged`(ホストは `TerminalTitle` コンポーネントを読む)。
- 旧 `default_bg: [u8;3]` は新スキーマでは `Palette` 全体に置き換わっており、こちらは機能拡張である。

## 8. 推奨着手順

1. **§3.1〜3.3(pump の駆動系)** — これが直らない限り新スタックは「文字が映らない端末」であり、他のすべての検証がここで止まる。
2. **§3.7(読み取りアクセサ)と §7(`vt_mut`)** — テストとホスト移行の前提。
3. **§3.4 / 3.5(マウスルーティング)** — 旧 `buttons.rs` / `wheel.rs` は Bevy 非依存の純関数(計 ~1,150 行 + テスト)なので、ほぼそのまま `orzma_term` へ移植可能。§4.3 の `SelectionKind` 拡張を同時に行う。
4. **§3.6(vi 結線)** — `VtBackend` への motion API 追加(orzma_vt 側の設計判断)と observer 実装。
5. **§4(webview 契約)** — `history_base` の「別アプローチ」設計を確定してから、§4.1 のアンカー機構を移植する。

## TODO

### Phase 1 — pump の駆動系(§3.1〜3.3)

- [x] `OrzmaTerm::pump` で `interpret` の `DamageVerdict` を受けて `Coalescer::arm_or_extend` を呼び、PTY 出力からフレームが emit されるようにする(§3.1)
- [x] エコー即時化と bootstrap の状態を `Coalescer` に内包する(`last_input_at` タイムスタンプ + 150ms 期限、`bootstrap` フラグ、`observe_chunk` / `note_user_input` / `needs_bootstrap` / `settle_emit`)(§3.1。Codex レビュー反映済み: 判定は arm 前の状態で行い、消費は emit 成立時のみ)
- [ ] `send_key` / `send_mouse` / `send_paste` の PTY 書き込み**成功後**に `Coalescer::note_user_input` を呼ぶ(§3.1。orzma_vt `frame()` 完成後の結線 PR で)
- [ ] `feed_chunk` の `arm_or_extend` 直呼びを `observe_chunk` に置き換え、`FlushDecision::Now` で pump が同一呼び出し内に emit するようにする(§3.1。同上)
- [ ] `pump` の emit ゲートを `needs_bootstrap() || is_due(now)` にして初回スナップショットを保証し、emit 成立時は `disarm` でなく `settle_emit` を呼ぶ(§3.1。同上。既存 pump テストのフィクスチャに bootstrap の settle が必要)
- [ ] ChildExit を返す `pump` は deadline を待たず staged frame を強制 emit する(§3.2 派生。ホストが ChildExit で即 teardown しても最終出力が描画されるように)
- [x] `Pty` に exit 読み取り口を追加し、`pump` が `TermSignal::ChildExit` を一度だけ emit するようにする(§3.2)
- [ ] `pump` で `OrzmaVt::drain_replies_into` を呼び、DSR/DA 応答バイトを 1 回の `write_all` で PTY へ書き戻す(§3.3)

### Phase 2 — アクセサとテスト復旧(§3.7、§7)

- [x] `OrzmaTerm` に `pub fn vt_mut(&mut self) -> &mut OrzmaVt<B>` を追加し、`--tests` のコンパイル(E0599 ×9)を直す(§7)
- [ ] `OrzmaTerm` に `modes()` を再公開する(マウスルーティング・focus 報告ゲート用)(§3.7)
- [ ] `OrzmaTerm` に `selected_text()` を再公開する(コピー操作用)(§3.7)
- [ ] `OrzmaTerm` に `selection_kind()` を再公開する(vi の v/V トグル判定用)(§3.7)
- [ ] `OrzmaTerm` に `display_offset` / `history_size` の読み取り口を追加する(vi インジケータ `[offset/total]` 用)(§3.7)
- [ ] PTY 非接続端末向けの強制 emit API(旧 `flush_emit` / `_vt_only` 相当)を設計・追加する(§3.7)
- [ ] `change_selection_kind` に「アンカー不在で `false`(呼び出し側が新規開始へフォールバック)」の戻り値契約を復元する(§3.7)

### Phase 3 — マウスルーティング移植(§3.4、§3.5、§4.3)

- [ ] 旧 `wheel.rs` の `WheelAction::route` / `route_horizontal`(mouse-protocol / alt-screen SS3 矢印 / スクロールバックの三分岐、`lines_per_notch` = 3・fine = 1・cap = 8、alt → meta ビット寄せ)を `orzma_term` へ移植し、コメントアウトを解消する(§3.4)
- [ ] 旧 `buttons.rs` の `ButtonAction::route`(Shift バイパス、Alt+左 = Block、クリック数 1/2/3 → Simple/Semantic/Lines、遅延 ArmDrag、`ClearAndWriteToPty`、毎フレーム上限)を `orzma_term` へ移植する(§3.5)
- [ ] `SelectionKind` に `Block` / `Semantic` を追加し、`From<SelectionType>` の `todo!()` を解消する(§4.3)
- [ ] 旧ルーターのテスト群を移植先で維持する(§3.4、§3.5)

### Phase 4 — vi 結線(§3.6)

- [ ] `VtBackend` に vi motion API を追加し、`ViMotion` 語彙を `orzma_vt` へ移す(§3.6)
- [ ] `apply_vi_mode` observer を実装し、`OrzmaVt::switch_vi_mode` へ接続する(§3.6)
- [ ] `apply_vi_motion` observer を実装する(§3.6)
- [ ] 付随挙動を移植する: 相対スクロール時の vi カーソル画面行追従、`scroll_to_top/bottom` での vi カーソル移動、vi 終了時の live-tail スナップ(§3.6)

### Phase 5 — webview 契約(§4.1、§4.2)

- [ ] `history_base` / `history_size` の「別アプローチ」設計を確定し、`FrameSnapshot` のコメントアウトを解消する(§4.1)
- [ ] OSC 5379 アンカー機構を移植する: OSC バイト位置での刻印 + `frame_seq` 同時刻印、`?2026` フラッシュ後サンプリング、`history_base` 単調追跡とフォールド時の合成 unmount-all、飽和時の primary mount 拒否、alt-screen の `FixedScreen` アンカー(§4.1)
- [ ] スナップショットでの絶対 mode 状態の再同期手段(旧 `modes: Vec<String>` 相当)と「alt-screen 遷移は必ず Snapshot で届く」不変条件の置き場所を決める(§4.2)

### orzma_vt 側の実装時に併せて運ぶ仕様(§5)

- [ ] タイトルのサニタイズ(C0/C1/bidi/ゼロ幅除去、256 字上限 + `…`)
- [ ] OSC 7 の検証(`file://` 必須、ホスト名照合、`;` 再結合、バイト単位パーセントデコード、重複排除)と Unix 専用実装の解消
- [ ] OSC 5379 の検証限界(view_id 1..=128 字 `[A-Za-z0-9._-]`、rows 1..=200、cols 1..=400、C1 導入子拒否)
- [ ] mode diff の追跡 8 フラグとワイヤ文字列の互換維持
- [ ] `ClipboardStore`(OSC 52 write)のシグナル化
