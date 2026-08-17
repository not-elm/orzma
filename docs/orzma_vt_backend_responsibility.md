# OrzmaVt と Backend の責務境界に関する調査

調査日: 2026-08-16

## 1. 結論

`OrzmaVt` 側へバックエンド非依存の処理を移すことで、将来独自 Backend を実装するときの必須実装量は大きく減らせる。

現在は [`VtBackend`](../crates/orzma_vt/src/vt.rs) に必須メソッドが15個、[`VtSelection`](../crates/orzma_vt/src/vt.rs) に8個あり、独自 Backend は合計23個のメソッドを実装する必要がある。このうち selection、frame生成、damageの蓄積、hyperlink ID管理などは Alacritty 固有の処理ではなく、Orzma として一貫した挙動を提供すべき処理である。

推奨する最初の到達点では、Backend の必須操作を次の5種類まで整理できる。

1. PTYバイト列の解釈
2. gridのresize
3. viewportのscroll
4. Backend状態の取得
5. raw rowの取得

ただし、VTシーケンスの解釈、gridのreflow、damage検出、cursor・mode・paletteの元状態などは端末エミュレータ固有であり、`OrzmaVt` へ移すべきではない。それらまで移すと、`OrzmaVt` 自身が別のVTエミュレータを実装することになり、Backend抽象化の意味がなくなる。

## 2. 調査対象

主に以下を確認した。

- [`crates/orzma_vt/src/vt.rs`](../crates/orzma_vt/src/vt.rs)
- [`crates/orzma_vt/src/vt/alacritty.rs`](../crates/orzma_vt/src/vt/alacritty.rs)
- [`crates/orzma_vt/src/schema`](../crates/orzma_vt/src/schema.rs)
- [`crates/orzma_term/src/lib.rs`](../crates/orzma_term/src/lib.rs)
- 旧実装の [`crates/orzma_tty_engine/src/vt/frame_builder.rs`](../crates/orzma_tty_engine/src/vt/frame_builder.rs)
- [`docs/memo.md`](memo.md) に記載されたterminal core分離方針

## 3. 現在の構造

現在の `OrzmaVt<B>` は、Backendを保持しながら次の処理を担当している。

- Backendが報告したdamageの蓄積
- `DamageVerdict` の分類
- selectionやscrollなどの操作で発生したdamageのstage
- frame sequence番号の保持
- Backend APIを外部へ転送する薄いラッパー

damageの蓄積を `OrzmaVt` が担当している点は適切である。一方、公開APIの多くがBackendへの単純転送になっており、Orzmaの機能単位とBackendの実装単位がほぼ1対1になっている。

### 3.1 現在の必須API

`VtBackend` の必須メソッドは次の15個である。

| メソッド | 現在の役割 |
|---|---|
| `new` | gridサイズを指定してBackendを生成 |
| `display_offset` | viewportのscroll位置を取得 |
| `cursor` | write cursorをOrzma schemaへ変換 |
| `vi_cursor` | vi cursorをOrzma schemaへ変換 |
| `interpret` | PTYバイト列を解釈してdamageを返す |
| `drain_signals` | title、bellなどのsignalを取り出す |
| `drain_replies_into` | DSR/DAなどの応答を取り出す |
| `scroll` | viewportを移動 |
| `modes` | inputに必要なterminal modeを取得 |
| `resize` | gridをresize/reflow |
| `grid_size` | 現在のgridサイズを取得 |
| `switch_vi_mode` | vi modeを切り替える |
| `cell_at` | raw cellを取得 |
| `history_size` | scrollback行数を取得 |
| `palette` | 現在のpaletteを取得 |

`VtSelection` の必須メソッドは次の8個である。

| メソッド | 現在の役割 |
|---|---|
| `start_selection` | 指定cellからselectionを開始 |
| `start_selection_at_vi_cursor` | vi cursorからselectionを開始 |
| `update_selection` | selectionの移動端を更新 |
| `change_selection_kind` | selection粒度を変更 |
| `clear_selection` | selectionを削除 |
| `selection_range` | 正規化済みの選択範囲を取得 |
| `selection_kind` | 現在の選択粒度を取得 |
| `selected_text` | 選択文字列を生成 |

## 4. 現状の問題点

### 4.1 BackendなしではOrzmaのselection仕様が定まらない

selectionのanchor、cell side、範囲の正規化、選択文字列の生成をすべてBackendが担当している。この設計では、Backendごとにselectionの挙動が変わる可能性がある。

たとえば次の仕様を各Backendが個別に再現する必要がある。

- backward selectionのstart/end正規化
- `CellSide` による境界cellの包含判定
- wide characterとspacer cellの処理
- zero-width characterの連結
- wrapされた論理行で改行を挿入しない処理
- line selectionの末尾改行
- scrollback上の負のgrid line

これらは端末エミュレータの種類ではなく、OrzmaのUI・コピー操作として統一されるべき仕様である。

また、現在の `frame()` は `B: VtBackend + VtSelection` のimpl blockに置かれている。そのため、説明上は「selection非対応Backend」を許容しているにもかかわらず、そのBackendではframeを生成できない。

### 4.2 frame生成用のraw schemaが不足している

現在の `SourceCell` が持つ情報は以下だけである。

- grid上の位置
- foreground/background color
- hyperlink

一方、frameの `Run` を生成するには少なくとも次の情報が必要である。

- primary character
- zero-width characters
- style flags
- wide characterとspacerの区別
- wrapped lineの情報
- hyperlinkのsource IDとURI

旧 `frame_builder` では Alacritty の `Cell` と `Flags` を直接参照してこれらを処理していた。現在の `SourceCell` のままでは、frame生成もselected-text生成も `OrzmaVt` へ完全移行できない。

さらに、現在のAlacritty版 `cell_at()` は `Option<SourceCell>` を返す一方、範囲確認より前にgridを直接indexしている。無効な `GridPoint` に対して `None` ではなくpanicするため、traitの契約も明確ではない。

### 4.3 constructorがBackendの構成を制限している

`VtBackend::new(cols, rows) -> Self` は、すべてのBackendが次の条件を満たすことを要求する。

- gridサイズだけで生成できる
- 生成に失敗しない
- Backend固有の設定や依存オブジェクトを受け取らない

独自Backendでは、設定、profile、共有resource、辞書、allocatorなどを渡したい可能性がある。生成処理をtraitから外し、生成済みBackendを `OrzmaVt::from_backend(backend)` に渡す方が拡張しやすい。

Alacritty用の簡便なconstructorは `impl OrzmaVt<AlacrittyVtBackend>` または `AlacrittyVtBackend` 自身に残せる。

### 4.4 interpretと副作用の取得が分離している

一度のPTY入力から次の結果が同時に発生し得る。

- visual damage
- bell/title/clipboardなどのsignal
- DSR/DA reply bytes
- terminal modeの変化

現在は `interpret`、`drain_signals`、`drain_replies_into` が別々であり、Backend側が内部queueとdrainのライフサイクルを実装する必要がある。

`interpret` の戻り値を `BackendUpdate` にして一度に返せば、Backendは同期的に発生した結果をまとめて返せる。`OrzmaVt` が必要に応じてsignal/replyを内部queueへ保持すればよい。

なお、signalやreplyの生成自体はVTパーサー固有であるためBackendに残る。ここで削減できるのは、queueとdrain APIの実装責務である。

### 4.5 getterを束ねるだけでは実装量は減らない

`cursor()`、`modes()`、`palette()` などを単一の `state()` にまとめるとtraitのメソッド数は減る。しかし、独自BackendがそれらをOrzma schemaへ変換する作業は残る。

したがって、次の2種類を区別する必要がある。

- API形状の単純化: 複数getterを `BackendState` に束ねる
- 実装責務の削減: selection、frame生成、hyperlink管理などを `OrzmaVt` に移す

独自Backendの実装負荷を実際に下げるのは後者である。

## 5. 推奨する責務境界

| 処理 | 推奨所有者 | 理由 |
|---|---|---|
| PTYバイト列のVT解釈 | Backend | 使用するエミュレータ固有 |
| grid状態とreflow | Backend | 内部storageとparser stateに密接 |
| damageの検出 | Backend | grid全体の比較を避け、エミュレータのtrackerを利用するため |
| cursor・modes・paletteの元状態 | Backend | VT parserが管理する状態 |
| raw rowの提供 | Backend | 内部grid表現をOrzma共通表現へ変換する境界 |
| damageの蓄積とSnapshot/Delta判定 | `OrzmaVt` | Backend非依存で、すでに一部実装済み |
| frame sequence | `OrzmaVt` | rendererとのOrzma固有契約 |
| Row/Run生成 | `OrzmaVt` | renderer向けのOrzma固有wire形式 |
| hyperlink interning | `OrzmaVt` | Backendのsource IDをOrzmaのstable IDへ変換する処理 |
| selection状態と選択文字列 | `OrzmaVt` | Backend間で一貫させるUI仕様 |
| signal/replyのqueue | `OrzmaVt` | Backendは発生結果を返すだけでよい |
| PTY、child process、reader thread | `OrzmaTerm` | I/O層の責務 |
| coalescerとclock | `OrzmaTerm` | VTの状態機械ではなくruntime scheduling |
| Bevy eventへの変換 | `bevy_orzma_term` | ECS adapterの責務 |

## 6. 推奨Backend API

最初の到達点として、次のようなAPIが考えられる。

```rust
pub trait VtBackend {
    fn advance(&mut self, bytes: &[u8]) -> BackendUpdate;

    fn resize(&mut self, size: GridSize) -> Option<Damage>;

    fn scroll(&mut self, motion: Scroll) -> Option<Damage>;

    fn state(&self) -> BackendState;

    fn source_row(&self, line: GridLine) -> Option<SourceRow>;
}

pub struct BackendUpdate {
    pub damage: Option<Damage>,
    pub signals: Vec<VtSignal>,
    pub replies: Vec<u8>,
}

pub struct BackendState {
    pub size: GridSize,
    pub display_offset: DisplayOffset,
    pub history_size: u32,
    pub cursor: Cursor,
    pub vi_cursor: Option<ViCursor>,
    pub modes: VtModes,
    pub palette: Palette,
}
```

`new` はtraitから削除し、次の形で注入する。

```rust
impl<B: VtBackend> OrzmaVt<B> {
    pub fn from_backend(backend: B) -> Self;
}
```

この案では必須メソッド数は5個になる。ただし `BackendState` の構築には現在のgetterと同等の情報が必要であるため、実質的な工数削減の中心は `VtSelection` の削除とframe生成の移動である。

### 6.1 最小Backendを実装可能にする

damageの行単位最適化を必須にすると、独自Backendは初期段階から高度なtrackerを必要とする。最小実装では、非空入力や状態変更に対して常に `Damage::Full` を返しても正しく動作できるようにする。

その後、必要なBackendだけが `Damage::Delta` を返して最適化できる設計が望ましい。

同様に、次のhelper/defaultを提供できる。

- `BackendUpdate::unchanged()`
- `BackendUpdate::full_damage()`
- signalなしの空vector
- replyなしの空buffer
- scrollback非対応Backend向けの `history_size = 0`
- palette拡張非対応Backend向けのdefault palette

完全な端末互換性を保証する場合はこれらの機能が必要になるが、まず動作するBackendを作るための初期実装量は減らせる。

## 7. SourceRowに必要な情報

frame生成とselectionを `OrzmaVt` に移すため、Backend境界のraw schemaには少なくとも次の情報が必要である。

```rust
pub struct SourceRow {
    pub cells: Vec<SourceCell>,
    pub wrapped: bool,
}

pub struct SourceCell {
    pub text: String,
    pub width: u8,
    pub spacer: SpacerKind,
    pub style: CellStyle,
    pub fg: Color,
    pub bg: Color,
    pub hyperlink: Option<SourceHyperlink>,
}
```

実際の型は性能測定後にborrowed viewやiteratorへ変更できるが、最初から複雑なGATやvisitor APIを要求すると独自Backendの実装難度が上がる。まずは明確なowned schemaで契約を固め、その後にallocationが問題となった場合だけ最適化する方がよい。

行単位APIにする利点は次のとおりである。

- lineの範囲確認を一度だけ行える
- 1 frameあたり `rows * cols` 回の境界呼び出しを避けられる
- wrap情報をcellではなくrowの性質として表現できる
- Backend側で既存のrow iteratorを利用しやすい
- frame生成とselected-text生成で同じraw schemaを再利用できる

## 8. selectionをOrzmaVtへ移す方法

`OrzmaVt` にBackend非依存の `SelectionState` を持たせる。

```rust
struct SelectionState {
    anchor: SelectionEndpoint,
    moving: SelectionEndpoint,
    kind: SelectionKind,
}
```

`OrzmaVt` が次を実装する。

- selection開始・更新・解除
- `CellSide` を考慮した範囲確定
- start/endの正規化
- line selectionへの展開
- renderer向け `SelectionRange` の生成
- raw rowからの選択文字列生成
- selection変更時のfull damage stage

これにより `VtSelection` traitを削除でき、すべてのBackendでselection挙動が一致する。また、`frame()` を通常の `impl<B: VtBackend>` に移せる。

wide character、zero-width character、tab、wrapped lineの扱いは、現在Alacrittyの `selection_to_string()` が暗黙に提供している。そのため、移行時には現在のselectionテストをBackend非依存テストとして残し、同じ出力を保証する必要がある。

## 9. viewportとvi modeの扱い

### 9.1 最初の段階ではBackendに残す

次の処理は当面Backendに残す方が安全である。

- `scroll`
- `display_offset`
- `history_size`
- `vi_cursor`
- `switch_vi_mode`

Alacrittyではviewport scroll、vi cursor、outputによるgrid scroll、resize/reflowが連動している。これらを一部だけ `OrzmaVt` に移すと、同じ文字列を指していたcursorやselectionがずれる可能性がある。

### 9.2 将来的な移動

Backendが `BackendUpdate` で次のgrid変換情報を報告できるようになれば、viewportとvi cursorも `OrzmaVt` へ移せる。

- 新しくhistoryへ追加された行数
- scrollbackからevictされた行数
- resize/reflowによるpoint変換
- alternate screenへの切り替え

この段階では `scroll`、`display_offset`、`vi_cursor`、`switch_vi_mode` をBackendから削除できる可能性がある。必須操作は4個程度まで減るが、grid変換契約の設計とテストが必要になるため、初回のリファクタリングには含めない方がよい。

## 10. 移行手順

### Phase 0: 現在の未完成状態を解消する

- `frame()` の `todo!()` を解消する
- `history_size()` を実装する
- replyとsignalの未実装箇所を整理する
- 空の明示的hyperlink IDを置換する処理を修正する
- `cell_at()` の範囲外契約を明確にする

### Phase 1: raw grid境界を完成させる

- `SourceCell` を `SourceRow` 中心のschemaへ置き換える
- character、zero-width、style、wide/spacer、wrap情報を追加する
- Alacritty固有の変換を `vt/alacritty.rs` 内へ集約する
- raw schemaのBackend非依存テストを追加する

### Phase 2: frame生成をOrzmaVtへ移す

- `OrzmaVt` に `HyperlinkInterner` を保持させる
- raw rowから `Row` / `Run` を生成する
- snapshot/deltaの生成、damage消費、sequence更新を完成させる
- frame生成から `VtSelection` boundを外す

### Phase 3: selectionをOrzmaVtへ移す

- `SelectionState` を導入する
- selection操作とselected-text生成を移す
- `VtSelection` を削除する
- 現在のAlacritty selectionテストをBackend非依存のテストへ移す

### Phase 4: constructorとupdate APIを整理する

- `VtBackend::new` を削除する
- `OrzmaVt::from_backend` を追加する
- `OrzmaTerm` にBackendまたはBackend factoryの注入経路を追加する
- `interpret`、signal、replyを `BackendUpdate` に統合する
- getter群を `BackendState` に整理する

### Phase 5: Backend実装者向けの適合テストを用意する

- 小さなfake backendで `OrzmaVt` 自身の処理をテストする
- Backend共通のcontract testを再利用可能にする
- 最小Backendが常にfull damageを返しても動作することを確認する
- Alacritty固有テストとBackend共通テストを分離する

### Phase 6: viewport・vi mode移動を再評価する

- grid shift/eviction/reflow eventの契約を設計する
- viewportを `OrzmaVt` が保持する場合の性能と複雑性を比較する
- 十分な利点が確認できた場合のみ移動する

## 11. テスト方針

現在の `OrzmaVt` テストは多くが `AlacrittyVtBackend` を直接使用している。この状態では、テストが `OrzmaVt` の契約を検証しているのか、Alacrittyの挙動を検証しているのかが分離されていない。

次の3層に分けることを推奨する。

| テスト層 | 検証対象 |
|---|---|
| `OrzmaVt<FakeBackend>` | damage stage、frame、selection、hyperlink、sequence |
| Backend contract test | size、cursor、mode、raw row、scroll、damageの共通契約 |
| Alacritty adapter test | Alacritty型からOrzma schemaへの変換、固有のparser挙動 |

独自Backend実装者がcontract testを呼び出せるようにすると、ドキュメントだけを読んで細かな端末仕様を再現する負担を減らせる。

## 12. 現在の検証結果

調査時点で以下を実行した。

```text
cargo check -p orzma_vt
cargo check -p orzma_vt --no-default-features
cargo check -p orzma_term
cargo test -p orzma_vt --no-fail-fast
```

結果は以下のとおりである。

- `orzma_vt` はdefault featureあり・なしの両方でcheck成功。ただし未使用import、未使用field、到達不能コードのwarningがある。
- `orzma_term` はcheck成功。ただし未使用コードのwarningがある。
- `orzma_vt` のunit testは189件中175件成功、14件失敗。
- 失敗のうち13件は `OrzmaVt::frame()` 内の `todo!()` が原因。
- 残る1件は空の明示的hyperlink source IDが置換されない問題。
- `AlacrittyVtBackend::history_size()` と `drain_replies_into()` にも `todo!()` が残っている。
- `drain_signals()` は現在常に空iteratorを返している。

このため、現在のブランチは責務移動の途中であり、設計評価は可能だが新しいBackendを実用投入できる完成状態ではない。

## 13. リスクと注意点

### 13.1 raw schemaを大きくしすぎない

Alacritty内部型をそのまま模倣すると、別BackendがAlacritty固有の概念まで実装することになる。raw schemaにはframe生成、selection、vi modeに本当に必要な意味だけを定義する。

### 13.2 traitメソッド数だけを目標にしない

複数のgetterを一つのstructに束ねれば数字上は小さくなるが、実装者の作業量は変わらない。評価指標は次の方が適切である。

- Backendが保持しなければならない追加状態の量
- Orzma固有アルゴリズムをBackendが再実装する箇所数
- 最小Backendが動作するまでに必要なコード量
- contract testを通すための必須機能数

### 13.3 frame hot pathのallocation

owned `SourceRow` は実装しやすい一方、frameごとにcellを複製する。まず正しい責務境界を確立し、profilingで問題が確認された場合にだけborrowed rowやvisitorへ変更する。

### 13.4 Backend固有変換をschemaから分離する

現在は `Color`、`GridPoint`、`SelectionKind` などのschema moduleにAlacritty変換が置かれている。将来の独自Backendを明確に支援するなら、これらをAlacritty adapter module、あるいは別crateへ移し、core schemaをBackend非依存に保つことが望ましい。

## 14. selection独自実装の工数見積もり

### 14.1 見積もりの前提

Rustとterminal gridの実装に慣れた開発者1名が担当し、レビュー待ちの時間を含めない人日で見積もる。現在の `OrzmaVt` が公開しているselection種別は `Simple` と `Lines` の2種類であり、旧実装で利用していた `Block` と `Semantic` は現在のschemaではコメントアウトされている。

また、独自実装とは単にanchorとmoving endを保持することではない。少なくとも次をAlacritty非依存で実装する必要がある。

- `CellSide` を考慮した範囲計算と前後方向の正規化
- wide character、spacer、zero-width character、tab、wrapped lineを考慮した文字列抽出
- outputによるgrid scroll、scrollback eviction、resize/reflow、alternate screen切替への追従またはselectionの無効化
- vi cursor移動との同期
- selection変更時のdamage通知

Alacritty 0.26.0では、selection本体が `selection.rs` の約380行、選択文字列の生成が `term/mod.rs` の約100行に分かれている。さらにselectionの回転・破棄処理がresize、scroll、erase、alternate screen切替など複数のgrid更新箇所へ組み込まれている。このため、座標計算単体よりもgrid更新との結合部分が主な工数になる。

### 14.2 現在の機能範囲を維持する場合

`Simple` と `Lines` のみを対象にし、現在のOrzmaVtの挙動を維持する場合の内訳は次のとおりである。

| 作業 | 目安 |
|---|---:|
| `SelectionState`、anchor/moving end、`CellSide`、範囲正規化 | 1〜2人日 |
| `Simple` / `Lines` の範囲生成 | 1〜2人日 |
| wide・zero-width・tab・wrap対応の選択文字列生成 | 2〜3人日 |
| scroll・history eviction・resize/reflow・alternate screenの連携 | 3〜5人日 |
| vi mode・damage・frameとの統合 | 1〜2人日 |
| Backend非依存テストへの移植、境界テスト追加、修正 | 2〜3人日 |

一部は並行して実装できるため、合計の現実的な見積もりは **10〜15人日** である。カレンダー上は、レビューと不具合修正の余裕を含めて **2〜3週間** を確保するのが安全である。

ただし、Phase 1の `SourceRow` / `SourceCell` 拡張とgrid変換eventの契約が先に完成している場合、selection固有の作業は **7〜10人日** 程度まで下げられる。このraw schema整備はframe生成にも必要なので、selectionだけのコストとして二重計上すべきではない。

現在のselectionテストは3ファイルに22件あり、基本的な範囲、vi mode、history上の負のline、wide文字、wrap、alternate screenを確認している。ただし、これらは `AlacrittyVtBackend` を直接使っており、scroll region、history eviction、resize/reflow、eraseとの組合せを十分には網羅していない。独自実装時は、同じテストを `OrzmaVt<FakeBackend>` のcontract testへ移すだけでなく、grid mutationごとのテスト追加が必要になる。

### 14.3 旧実装相当の `Block` / `Semantic` まで戻す場合

旧 `orzma_tty_engine` はAlt+clickの `Block` selectionとdouble clickの `Semantic` selectionをAlacrittyへ委譲していた。これらもOrzma側で独自実装する場合は、現在範囲に対してさらに **5〜8人日** を見込む。

- `Block`: 矩形範囲、行ごとの右端trim、wide文字との境界処理
- `Semantic`: semantic escape characters、単語境界探索、括弧対応探索、履歴をまたぐ探索
- 2種の操作・文字列抽出・scroll/reflowに関する追加テスト

したがって、raw schema整備を含む完全版の合計は **15〜23人日**、カレンダー上は **3〜5週間** が目安になる。

### 14.4 推奨する進め方

最初から4種類すべてを実装せず、まず `Simple` / `Lines` に限定する。特に、selection自身がBackendの内部gridを監視する設計にはせず、Backendが `GridMutation` のような形で次の構造変化をOrzmaVtへ通知する契約を先に決めるべきである。

- scroll regionと移動量
- historyへの追加量とeviction量
- resize/reflow前後のpoint変換、またはselection無効化指示
- primary/alternate screenの切替
- eraseされたline範囲

この通知契約がなければ、`OrzmaVt` のselectionは表示上の座標だけを保持することになり、出力後に別の文字列を指す不具合が発生する。逆にこの契約を明確にすれば、Backendはselectionアルゴリズムを実装せず、gridに起きた事実だけを報告すればよくなる。

## 15. selectionを別構造体へ分離するAPI案

selectionは `OrzmaVt` のフィールドとして直接ばらばらに保持するのではなく、純粋な状態機械に近い `SelectionModel` へまとめるのが扱いやすい。`SelectionModel` はBackend型を保持せず、anchor、移動端、selection種別だけを所有する。

### 15.1 selection自身が所有する状態

```rust
pub struct SelectionModel {
    active: Option<ActiveSelection>,
}

struct ActiveSelection {
    anchor: SelectionEndpoint,
    head: SelectionEndpoint,
    kind: SelectionKind,
}

#[derive(Clone, Copy)]
struct SelectionEndpoint {
    point: GridPoint,
    side: CellSide,
}

#[must_use]
pub enum SelectionChange {
    Unchanged,
    VisualChanged,
}
```

selectionのanchorを個々の `SourceCell` に埋め込む必要はない。anchorは「現在のユーザー操作の状態」であり、terminal gridの内容ではないためである。cellは文字、幅、wrap、styleなどのraw terminal情報に限定し、selectionは `GridPoint` でcellを参照する。

### 15.2 公開操作

```rust
impl SelectionModel {
    pub fn new() -> Self;

    pub fn begin(
        &mut self,
        point: GridPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> SelectionChange;

    pub fn extend(
        &mut self,
        point: GridPoint,
        side: CellSide,
    ) -> SelectionChange;

    pub fn change_kind(&mut self, kind: SelectionKind) -> SelectionChange;
    pub fn clear(&mut self) -> SelectionChange;

    pub fn kind(&self) -> Option<SelectionKind>;

    pub fn range(
        &self,
        grid: &impl SelectionGrid,
    ) -> Option<SelectionRange>;

    pub fn text(
        &self,
        grid: &impl SelectionGrid,
    ) -> Option<String>;

    pub fn apply_grid_mutation(
        &mut self,
        mutation: GridMutation,
    ) -> SelectionChange;
}
```

`begin`、`extend`、`change_kind`、`clear` は状態変更だけを行う。`range` がendpointの前後関係、`CellSide`、`Simple` / `Lines` を解釈してrenderer向けの正規化済み範囲を生成し、`text` が同じ範囲からコピー文字列を生成する。

操作結果を `SelectionChange` で返すことで、`SelectionModel` 自身はdamage型を知らず、呼び出し側の `OrzmaVt` が必要なdamageをstageできる。

### 15.3 gridを読むための最小インターフェース

```rust
pub trait SelectionGrid {
    fn size(&self) -> GridSize;
    fn history_size(&self) -> u32;
    fn row(&self, line: GridLine) -> Option<SourceRow>;
}

pub struct SourceRow {
    pub cells: Vec<SourceCell>,
    pub wrapped: bool,
}
```

`SelectionGrid` はselectionが必要とする読み取り専用viewである。Backend固有のcursor、palette、parser、damage trackerなどは公開しない。`SourceCell` からは少なくとも文字列、文字幅、wide spacer、tab、zero-width文字を判定できる必要がある。最初は実装しやすいowned `SourceRow` とし、profilingでallocationが問題になった場合だけborrowed viewへ変更する。

実際には `VtBackend::source_row()` を使う小さなadapterを `OrzmaVt` 内に置けばよく、各Backendにselectionアルゴリズムを実装させる必要はない。

### 15.4 grid変化の通知

```rust
pub enum GridMutation {
    /// scrollなどにより、範囲内のlineがdeltaだけ移動した。
    LinesMoved {
        start: GridLine,
        end: GridLine,
        delta: i32,
    },

    /// eraseやhistory evictionにより、この範囲の内容が失われた。
    LinesInvalidated {
        start: GridLine,
        end: GridLine,
    },

    /// alternate screen切替やcolumn変更を伴うreflowなど。
    Reset,
}

pub struct BackendUpdate {
    pub damage: Option<Damage>,
    pub grid_mutations: Vec<GridMutation>,
    pub signals: Vec<VtSignal>,
    pub replies: Vec<u8>,
}
```

`LinesMoved` では両endpointを同じだけ移動し、移動後にgrid外へ出た場合はclampまたはclearする。`LinesInvalidated` が選択範囲と交差した場合はselectionをclearする。最初の実装では、column数が変わるresize/reflowとalternate screen切替を `Reset` としてselectionをclearすればよい。reflow後もselectionを維持する仕様が必要になった場合だけ、将来point変換情報を追加する。

### 15.5 `OrzmaVt` との接続

```rust
pub struct OrzmaVt<B> {
    backend: B,
    selection: SelectionModel,
    // damage、frame sequence、hyperlink internerなど
}

impl<B: VtBackend> OrzmaVt<B> {
    pub fn start_selection(
        &mut self,
        point: GridPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> bool {
        let changed = self.selection.begin(point, side, kind);
        self.stage_selection_change(changed)
    }

    pub fn start_selection_at_vi_cursor(
        &mut self,
        kind: SelectionKind,
    ) -> bool {
        let Some(cursor) = self.backend.vi_cursor() else {
            return false;
        };
        let changed = self.selection.begin(cursor.point, CellSide::Left, kind);
        self.stage_selection_change(changed)
    }

    pub fn interpret(&mut self, bytes: &[u8]) {
        let update = self.backend.advance(bytes);

        if let Some(damage) = update.damage {
            self.stage(damage);
        }
        for mutation in update.grid_mutations {
            let changed = self.selection.apply_grid_mutation(mutation);
            self.stage_selection_change(changed);
        }

        // signalとreplyもOrzmaVt側のqueueへ移す。
    }

    pub fn selection_range(&self) -> Option<SelectionRange> {
        let grid = BackendGridView::new(&self.backend);
        self.selection.range(&grid)
    }

    pub fn selected_text(&self) -> Option<String> {
        let grid = BackendGridView::new(&self.backend);
        self.selection.text(&grid)
    }

    fn stage_selection_change(&mut self, change: SelectionChange) -> bool {
        match change {
            SelectionChange::Unchanged => false,
            SelectionChange::VisualChanged => {
                self.stage(Damage::Full);
                true
            }
        }
    }
}
```

この構成では、現在の `VtSelection` の8メソッドをBackendから削除できる。Backendがselectionのために行うのは、raw rowを読めるようにすることと、gridの構造変化を `BackendUpdate` で報告することだけになる。

`SelectionModel` のunit testはfake `SelectionGrid` を使ってBackendなしで実行でき、Alacritty adapter testでは `GridMutation` とraw rowへの変換だけを検証できる。これによりselectionの端末共通仕様とBackend固有動作を分離できる。

## 16. 最終提案

現在進められている「Backendの `extract_rows` をraw cell取得へ変え、frame生成を `OrzmaVt` に移す」方向は妥当である。ただし、現在の `SourceCell` では情報が不足しているため、まず `SourceRow` 契約を設計する必要がある。

最も効果が大きい変更は、次の3点である。

1. frame・Row/Run生成・hyperlink interningを `OrzmaVt` が担当する。
2. selectionを完全に `OrzmaVt` へ移し、`VtSelection` を削除する。
3. constructor注入と `BackendUpdate` を導入し、Backendを「VT状態機械とraw gridのadapter」に限定する。

これにより、独自BackendはOrzmaのUI機能やrenderer向けwire形式を再実装せず、使用するVTエミュレータを共通のraw schemaへ変換することに集中できる。
