# webview 配置識別子を host 採番の `InstanceId` に一本化する

`PlacementId` と `instance_id` を削除し、コントロールプレーンが払い出す
`InstanceId` 1本に統合する設計書。前提となる作業（APC ディスパッチの実装、
SDK の APC 移植、仕様書の同期）は PR #269 で完了している。

## 現状: 識別子が3層ある

| 識別子 | 誰が採番 | いつ | クライアントに返るか |
| --- | --- | --- | --- |
| `view_id`（handle） | host（コントロールプレーン、128bit 乱数 base32） | `register` 時 | 返る（コントロールソケット） |
| `instance_id` | クライアント | mount を書く前（任意） | 返らない |
| `PlacementId(u64)` | VT | mount を処理した後 | 返らない（host にのみ渡る） |

`view_id` と `handle` は**同じ文字列**で、層ごとに名前が割れているだけである
（`docs/orzma_webview_protocol.md` が「`view_id` — the handle from `register`」と
定義し、`resolve_mount` は受け取った `view_id` をそのまま handle キーの registry に
引いている）。

2つのアドレス空間が並走しているのが問題の核である。

- `(view_id, instance_id)` — VT の supersede / unmount のアドレス。
- `PlacementId` — フレームが運ぶ投影のアドレス、および eviction のアドレス。

`instance_id` は現時点で誰も使っていない。SDK の `mount()` に instance 引数がなく
（`sdk/ratatui_orzma/src/escape.rs` は `n=` を一切出さない）、`Session` は focus 操作に
常に `instance: None` を送る。`apps/` 配下にも使用箇所はない。

現状のワイヤ:

```text
ESC _ Omount;v=<handle>,r=<n>,c=<n>[,n=<instance>] ESC \
ESC _ Ounmount[;v=<handle>[,n=<instance>]] ESC \
```

## 何が問題か — 採番の方向が逆

`instance_id` と `PlacementId` は名前が違うだけの2つの id ではない。**誰がいつ
決めるか**が逆になっている。

- `instance_id` はクライアントが mount を書く前に決めるので、後の `unmount` で
  自分で指名できる。
- `PlacementId` は VT が mount を処理した後に採番する。APC は一方通行
  （プログラムが PTY に書くだけ）なので、クライアントは自分の placement の id を
  知る手段がない。

したがって「`PlacementId` だけにして `unmount` もそれで指名する」は、現行プロトコル
のままでは成立しない。`instance_id` が存在する理由はここにある。

## 先行事例調査

kitty の仕様原文（rst ソース）と kitty / Ghostty / WezTerm の実装ソースを
突き合わせた結果。

**kitty の `p=`（placement id）はグローバルではない。** 仕様が明記している —
"Every placement is uniquely identified by the **pair** of the `image id` and
the `placement id`." つまり per-image スコープで、`p=7` は image 10 と image 11
で別物。削除も必ず image id 起点（`d=i,i=10,p=7`）で、`p=` 単独でアドレスする
削除は kitty に存在しない。

**「端末が採番してクライアントに返す」経路は kitty に実在するが、返るのは
image id であって placement id ではない。** `I=`（image number）を送ると端末が
`i=`（image id）を採番して `<ESC>_Gi=99,I=13;OK<ESC>\` で返す。これは orzma の
`register → {ok, handle}` と構造的に同型で、違いは kitty が帯域内、orzma が
帯域外（Unix ソケット）という点だけである。

**衝突が問題になるのは container id の層であって、その下の placement id ではない。**
kitty の "Since IDs are in a global namespace there can easily be collisions" という
一文は、Unicode プレースホルダで **image ID を前景色に詰め込む**際のビット幅の話
（256色モードで8bit、truecolor で24bit）であり、解決策として示されているのは3つ目の
diacritic である。クライアント採番の placement id を否定した文ではない。`I=` の推奨は
別の節にあり、対象は image id である。orzma は handle を host 採番の 128bit にしたことで、
kitty が `I=` で解いたのと同じ問題を同じ層で既に解いている。

**Ghostty が内部 placement id を持つ理由は、orzma には存在しない制約である。**
ソースが明言している —

```zig
// The important piece here is that the placement ID needs to
// be marked internal if it is zero. This allows multiple placements
// to be added for the same image.
```

1つの image に**匿名 placement を N 個**持つためである。orzma の `instance_id` は
`Option` で、`(view_id, None)` は匿名インスタンス1つを指すため、キーは既に全域的で
内部 id を必要としない。

なお `next_internal_placement_id` は **0 から始まる** — "This is never user-facing so we
can start at 0. ... any number is valid." と書かれている。`2147483647` から始めて
「クライアントが選びがちな低域を避ける」のは隣のフィールド `next_image_id`（＝orzma の
handle に相当し、既に host 採番）のコメントである。

| | placement のアドレス | 端末が内部 id を持つか |
| --- | --- | --- |
| kitty（仕様） | `(image id, placement id)` の組。placement id はクライアントが `1..=4294967295` から選ぶ | — |
| Ghostty | 同左 | 持つ（匿名 placement を N 個許すため） |
| WezTerm | `HashMap<(u32, Option<u32>), PlacementInfo>` — 組をそのままキーにする | **持たない** |
| iTerm2 / Sixel | placement という概念自体がない | — |

**「端末が採番したグローバル一意な placement id を唯一の wire アドレスにしている実装」は
見つからなかった。** 逆に「サーバが払い出した親 id の下でクライアントが子 id を決める」
構造は広く実在する — kitty の `(image id, placement id)`、WezTerm の実装、X11 の
`resource-id-base` / `resource-id-mask`、Wayland のクライアント／サーバ id レンジ分割、
HTTP/2 の奇数／偶数ストリーム ID、Google AIP-133（親はサーバ、子 id はクライアント指定）。

## 設計判断

### D1. `register` の応答に instance を同梱する

```text
C→S {"op":"register","kind":"inline","html":"…"}
S→C {"ok":true,"handle":"nf2k7q5w…","instance":"3f5a9c02d1e84b7690ab3cde12f45678"}

C→S {"op":"new_instance","handle":"nf2k7q5w…"}      ← 2配置目以降だけ
S→C {"ok":true,"instance":"81b4e77c05a3492fd6180e29ba735fc1"}
```

1配置しか要らない大多数のケースは往復1回のまま。`mount` を常に `n=` 必須にでき、
VT から「デフォルトインスタンス」という特例が消える。

### 採用理由は1つだけである

**`orzma_vt` から handle の概念を完全に排除するため。** 一本化後の VT の placement は
`{ id, anchor, col, size }` だけになり、コンテンツ登録・オリジン・RPC ルーティング・
所有権に使われる handle を VT は一切知らなくなる。`orzma_vt` を webview 登録モデルから
独立した汎用 VT として保つことが、この設計の最上位要件である。

**次の理由は成立しないので、根拠として使ってはならない**（いずれも調査で反証済み。
詳細は「却下した案」節の A / A' を参照）:

- ❌「衝突を防ぐため」— `(handle, instance)` の組は handle が host 採番の 128bit で
  あるため構造的に衝突しない。クライアント採番でも衝突はゼロである。
- ❌「識別子を1つにするため」— D1 は識別子を1つにしない。handle はコンテンツ識別子
  として残るので、アドレス（`instance`）＋コンテンツ id（`handle`）の2つになる。
- ❌「VT が内部 id を持ち続けずに済む唯一の方法だから」— D3 が非再利用の要求を
  取り下げた時点で、クライアント採番でも内部 id は不要になった。

### この設計が引き受けるコスト

以下は設計2（`(handle, instance)` の組をアドレスにする案）なら発生しないもので、
D1 を選ぶ以上、承知のうえで払う対価である。

| コスト | 内容 |
| --- | --- |
| 既存呼び出しの書き換え | `WebviewWidget::new` の呼び出し16箇所が全て書き換え対象。**D8 の `HandleId` によりコンパイルエラーとして現れる**ので、無言スキップにはならない（D8 導入前は unit test も green のまま通り抜ける経路だった） |
| 描画ループのブロック | 2配置目以降は `new_instance` のソケット往復が要る。セットアップ時のみ呼ぶ制約が付く |
| reconnect の往復 | registration あたり 1 + N 回（設計2 は 1 回） |
| 新設物 | registry の逆引きマップ、`RemovedRegistration`、`mint_instance`、`ClientMsg::NewInstance`、`ServerMsg` のバリアント分割、SDK の `Pending` enum |
| id 形式の固定 | `InstanceId(u128)` + `^[0-9a-f]{32}$` が host の id 幅を `orzma_vt` の公開型と APC 文法に焼き込む。host は採番方式を変えられなくなる |

### D2. mount APC から `v=` を落とし、`n=` だけにする

```text
ESC _ Omount;n=<instance>,r=<n>,c=<n> ESC \    既に live なら in-place 更新
ESC _ Ounmount;n=<instance> ESC \              その配置だけ外す
ESC _ Ounmount ESC \                           この端末の全配置
```

instance → handle の対応は host が持つので `v=` は冗長。結果として `orzma_vt` から
handle の概念が完全に消え、placement は `{ id, anchor, col, size }` だけになる。
v/n の不整合というエラークラスも消滅する。

失われるのは `unmount;v=<handle>`（その handle の全インスタンス）という中間スコープ。
これは D7 の host→VT 削除経路で置き換える。

### D3. `InstanceId` は恒久スロット名

mint 後、handle が登録されている間ずっと有効。`unmount` → 同じ id で再 `mount` は
正当である。SDK の差分 flush（レイアウトが変わるたび mount / 消えたら unmount）が
socket 往復なしで回る。

注意: **`unmount` → 再 `mount` はエンティティとブラウザを作り直す**ので、ページの
状態は保存されない。in-place になるのは、live な instance へ再 mount した場合だけ
である（下の D3 不変条件2）。

`PlacementId` の doc が宣言していた「単調増加・セッション中再利用なし」
（`crates/orzma_vt/src/placement.rs`）は、次の2つに置き換わる。

1. **allocated な `InstanceId` は host の registry 全体で一意。** mounted かどうかは
   関係ない。unmount 済みでも handle 存続中は再 mount 可能＝allocated なので、
   instance → handle の逆引きが一意である必要がある。根拠は 128bit の OS CSPRNG
   であり、数千エントリ対 2^128 の衝突確率は到達不能である。この一意性は構造的では
   なく確率的であり、mint 時の `debug_assert!`（§3.1）はその前提が破れたことを
   デバッグビルドで捕まえる tripwire であって、リリースビルドで一意性を強制する
   ものではない。
2. **ある瞬間に live な `InstanceId` は端末内で一意。** `supersede` が両スクリーンを
   走査するので既存挙動のまま満たされる。端末間は、instance が handle に属し handle が
   `owner_surface` に属することから、既存の所有権ゲートが担保する。

**遅延した eviction が後継を殺さない根拠**は、supersede が先にテーブルから消すことである。
`mount_placement` は cap チェックの前に両スクリーンで `supersede_placement` を走らせるので、
再 mount された id の旧エントリはテーブルから消えており、**`evict_lost_anchors` が
superseded な id を名指しすることは原理的にありえない**。この論証は alternate screen の
flip、resize による anchor の孤立、history trim、RIS、複数チャンクが1 pump に入る場合を
一様にカバーし、signal の順序に依存しない（resize と history trim はテーブルを触らず、
anchor が解決しなくなるだけである）。

補足として、signal の順序自体も byte stream 順で保存される（`OrzmaTty::pump` は
drain_chunks で chunk の signal を byte 順に積んで 1 本で返し、各 chunk の末尾で
`Executor::sweep_evictions` が取り残しを `WebviewEvicted` として足す）。ただし
「eviction が必ず最後に来る」わけではない — alternate screen の flip は
`Executor::switch_screen`（`crates/orzma_vt/src/interpreter.rs`）が interpret 中にその場で
`WebviewEvicted` を出すので、`Evicted{X} → Mount{X}` の順序は実際に起こる。その順序が
安全に処理されることは §5 で保証されている。

### D4. 型は `InstanceId(u128)`、ワイヤは16進32桁固定

`^[0-9a-f]{32}$`。`format!("{:032x}")` と `u128::from_str_radix(s, 16)` だけで往復でき、
`orzma_vt` に新しい依存が増えない。`AnchoredPlacement` が `Copy` のままで、フレーム
毎の String アロケーションが発生しない。VT は「16進32桁でなければ malformed」という
構文ゲートを無償で得る。

**`u128::from_str_radix` 単体では `^[0-9a-f]{32}$` にならない** — 大文字と `+` 接頭辞を
受理するので、長さ32＋全バイトが `0-9a-f` であることを明示検査する。

JSON では数値にできない（u128 は JS の `Number` が 2^53 で壊れる）ので文字列として運ぶ。
10進（桁数可変・先頭ゼロのぶれ）と base32（`orzma_vt` に `data-encoding` 依存が増え、
handle と見分けがつかない）は却下した。

型の細部:

- **`Debug` は手で実装して16進を出す。** 導出すると10進で出るので、`tracing` の各行と
  `TtyWebview*Signal` の `Debug` がワイヤの綴りと食い違う。
- **`Default` は導出しない。** `InstanceId(0)` は `"0"×32` という正当なワイヤ値なので、
  事故で入った全ゼロ id が本物と区別できなくなる。
- `MAX_VIEW_ID` を削除しても長さの上限は失われない。`MAX_APC_LEN = 1024` がフィールド
  解析より前にペイロード全体を弾く（`crates/orzma_vt/src/interpreter/apc.rs`）。
- 16進ちょうど32桁の上限は `u128::MAX` と一致するので、**パースにオーバーフロー分岐が
  要らない**。構文ゲートは全域的になる。

### D5. `ServerMsg` はバリアントを分ける

既存の `Ok` は `handle: String` が必須なので `{"ok":true,"instance":"…"}` を表現できない。

```rust
#[serde(untagged)]
pub(crate) enum ServerMsg {
    Registered { ok: bool, handle: String, instance: String },
    Instanced  { ok: bool, instance: String },
    Err        { ok: bool, error: String },
}
```

host は serialize しかしないので untagged の曖昧さは発生しない。

### D6. SDK は `WebviewHandle` が既定 instance を兼ねる

```rust
let h  = session.register(wv)?;   // handle + 既定 instance
let h2 = h.new_instance()?;       // WebviewInstance

frame.render_stateful_widget(
    WebviewWidget::new(h.instance_id()), area_a, &mut placements);
frame.render_stateful_widget(
    WebviewWidget::new(h2.id()),         area_b, &mut placements);

h.emit("tick", &n)?;   // コンテンツスコープ: 両方のページに届く
```

`WebviewWidget::new` は `impl Into<String>` のままとし、専用トレイトは導入しない。

**ただし既存コードは無修正では通らない。** 現在の呼び出し元は5箇所すべてが
`handle.id()`（base32）を渡している — `apps/orzmd/src/ui.rs`、`apps/orzbrowser/src/ui.rs`、
`sdk/ratatui_orzma/examples/{simple,rpc,forward_keys}.rs`。`^[0-9a-f]{32}$` ゲートでは
これらが `flush_placements` の防御スキップに落ち、**コンパイルエラーも警告もなく webview が
出なくなる**。したがって:

- 全呼び出し元を `handle.instance_id()` に書き換えることを、この変更の必須作業に含める。
- `flush_placements` のスキップに `tracing::debug!` を足し、無言で落ちないようにする（§4.3）。

型で塞ぐ案（`WebviewWidget::new(&WebviewInstance)` か `InstanceId` newtype）は、この5箇所を
コンパイルエラーに変えられる。採用しないのは設計判断であり、代償は上の2項目で埋める。

### D8. handle も newtype にする（`HandleId`）

`handle` という名前は維持する。改名（`ViewId` 案）は検討して却下した — 理由は
「却下した案」節の E を参照。代わりに**型で分ける**:

```rust
// host 側（bevy_orzma_webview）
pub struct HandleId(String);   // pub — Webview.handle が pub でルートバイナリが構築するため

// SDK 側（ratatui_orzma、orzma_vt に依存しない独立クレートなので自前で定義）
pub struct HandleId(String);
```

**`From<HandleId> for String` は実装しない。** これが型ゲートの本体である。
`WebviewWidget::new(impl Into<String>)` が handle を受け付けなくなるのは、この変換が
存在しないからで、便利さのつもりで足すと塞いだ穴が黙って開く。`Display` は実装してよい
（`ToString` は `Into<String>` を生やさないため）。`orzma://<handle>/` の構築も
`format!` 経由で従来どおり書ける。

serde は `#[serde(transparent)]` にして、ワイヤ表現は文字列のまま変えない。

**効き目は SDK 側に集中する。** host 側では `InstanceId(u128)` と handle の `String` が
既に別型なので取り違えは起こらず、newtype の価値は自己文書化
（`HashMap<InstanceId, HandleId>` が「instance → handle」だと型で読める）にとどまる。
一方 SDK では `id() -> String` と `instance_id() -> String` が同型で、実際に
`WebviewWidget::new` の16箇所が無言で壊れる経路になっていた。`HandleId` はそれを
コンパイルエラーに変える。

instance 側は SDK でも `String` のままにする。事故の向きは常に「instance を期待する場所に
handle を渡す」であり、その一方向を塞げば十分だからである。両方を newtype にすると
API 表面が増えるだけで、防げる誤りは増えない。

### D7. host→VT の placement 削除経路を新設する

現在の `despawn_mounted`（`crates/bevy_orzma_webview/src/control_plane.rs`）は ECS の
子を despawn するだけで、VT の placement table は無傷である。**12枠を mount して
`unregister` すると cap が枯れたままになる**（既存バグ）。`unmount;v=` を落とす D2 と、
multi-instance が実用になることの両方から、この設計に含める。

```rust
#[derive(EntityEvent)]
pub struct RequestTtyWebviewRemove {
    #[event_target] pub terminal: Entity,
    pub instances: Vec<InstanceId>,
}
```

## §1 識別子モデルとワイヤ

識別子は1つになる。`InstanceId` が「クライアントが持つアドレス」「VT が保持する id」
「フレームが運ぶ id」「エンティティの identity」を兼ね、`handle` はコンテンツの
識別子（`orzma://<handle>/` オリジン、RPC ルーティング、所有権）に純化される。

APC の面:

| 綴り | 意味 |
| --- | --- |
| `Omount;n=<inst>,r=<n>,c=<n>` | その instance をカーソル位置に配置。既に live なら in-place 更新 |
| `Ounmount;n=<inst>` | その配置だけ外す |
| `Ounmount` | この端末の全配置を外す |

unmount のキーが1つになるので、「`n=` は `v=` を見た後でのみ受理」という現在の
順序依存が消え、**unmount のキーも順序独立**になる。

## §2 `orzma_vt` の変更

**命名方針**: 変わるのは識別子の所有者だけで、「placement = 端末に貼られた矩形」と
いう概念語は残す。`AnchoredPlacement` / `PlacementSize` / `ScreenPlacements` /
`MAX_PLACEMENTS` は改名しない。`InstanceId` は placement を指す id なので
`placement.rs` に置く。

### 2.1 `placement.rs`

```rust
pub struct InstanceId(pub u128);

impl InstanceId {
    /// Digits in this id's wire spelling.
    pub const WIRE_DIGITS: usize = 32;
    pub fn from_bytes(bytes: [u8; 16]) -> Self { /* u128::from_be_bytes */ }
}

impl FromStr for InstanceId { /* 長さ Self::WIRE_DIGITS + [0-9a-f] を明示検査 */ }
impl Display  for InstanceId { /* {:0width$x}, width = Self::WIRE_DIGITS */ }
impl Debug    for InstanceId { /* 導出だと10進になるので手で hex を出す */ }
```

桁数は `InstanceId::WIRE_DIGITS` を唯一の出所にする。`from_str` の長さ検査と `Display` の
ゼロ埋め幅が別々のリテラルだと、片方だけ変えたときに黙って食い違う。

`PlacementId(u64)` は削除。`AnchoredPlacement.id: InstanceId` で `Copy` は維持。
`PlacementId` の doc の `# Invariants` は D3 の2条件に差し替える。

**`AnchoredPlacement` の `# Invariants` も書き換える。** 現在は「`size` は常に `id` の
mount 時の予約と一致する。VT はサイズ変更を新しい id での remount として扱う」と宣言し、
`size` の field doc も「unchanged from the mount that reserved it」と書いている。id 一本化後は
サイズ変更が id を保ったまま起きるので、この宣言は偽になる。実際に壊れる消費者はいない
（host は毎フレーム size を読み直し、`diff_placements` は `Vec` の位置比較なのでサイズ変更で
再 emit される）が、doc は直す必要がある。

### 2.2 placement テーブルの縮小

```rust
struct Placement {
    id: InstanceId,
    anchor: LineId,
    col: GridColumn,
    size: PlacementSize,
}
```

`view_id` / `instance_id` の2フィールドと `Placement::addressed_by` が消える。

| メソッド | 現在 | 変更後 |
| --- | --- | --- |
| `ScreenPlacements::mount` | `(id, anchor, col, size, view_id, instance_id)` | `(id, anchor, col, size)` |
| `ScreenPlacements::supersede` | `(view_id, Option<instance_id>)` | `(id: InstanceId)` |
| `ScreenPlacements::unmount` | `(Option<view_id>, Option<instance_id>)` の3分岐 | `(Option<InstanceId>)` の2分岐 |
| `ScreenPlacements::remove_many` | — | 新設 `(&[InstanceId]) -> bool` |

`DeviceState` からは `next_placement_id` フィールドと `mint_placement_id()` が消える
（VT はもう採番しない）。`mount_placement` の戻りは `Option<PlacementId>` から
`bool`（cap が受け入れたか）になる。`DeviceState::remove_placements(&[InstanceId]) -> bool`
は両スクリーンを走査し、既存の `unmount_placement` と同じく短絡しない。

### 2.3 APC パーサ

```rust
enum WebviewApcRequest {
    Mount   { instance: InstanceId, size: PlacementSize },
    Unmount { instance: Option<InstanceId> },
}
```

`mount` の必須キーは `n` / `r` / `c` の3つ。`v=` は未知キーとして malformed 扱い。
`valid_view_id()` と `MAX_VIEW_ID` は削除できる（handle がワイヤから消え、
`InstanceId::from_str` が唯一のゲートになる）。

### 2.4 signal

```rust
WebviewMount         { instance: InstanceId, size: PlacementSize },
WebviewMountRejected { instance: InstanceId },
WebviewUnmount       { instance: Option<InstanceId> },
WebviewEvicted       { placements: Vec<InstanceId> },
```

`view_id` が全バリアントから消え、`WebviewMount` の `placement` フィールドも消える
（`instance` がそれを兼ねる）。

### 2.5 host 駆動の削除（`Vt` トレイトのメソッド）

D7 の受け口は `Vt` トレイトに載せる。`remove_placements` はトレイト上の placement 専用
メソッドであり、退避の報告は `interpret` と `resize` が各自の戻り値で行う
（`docs/todo/eviction-at-source.md`）。

```rust
// orzma_vt — Vt トレイト
/// Removes the placements the host names, on either screen; returns
/// whether anything went.
fn remove_placements(&mut self, instances: &[InstanceId]) -> bool;

// orzma_tty — ジェネリックのまま
impl<V: Vt> OrzmaTty<V> {
    pub fn remove_placements(&mut self, instances: &[InstanceId]) {
        if self.vt.remove_placements(instances) {
            self.coalescer.arm_or_extend(Instant::now());
        }
    }
}
```

代償: `orzma_tty` のテスト用 `FakeVt` にスタブが1つ増える。`pub mod test_support` は
無条件公開なので、そのスタブは `orzma_tty` の公開 API に出る。

### なぜこの経路が必要か

発火点は4つあるが、**本質的に必要なのは下の2つ**である。

| 発火点 | クライアント | 他の手段で代替できるか |
| --- | --- | --- |
| `unregister`（UDS op） | 生きている | できる（SDK が先に `Ounmount;n=` を書けばよい） |
| **接続断 / プロセス死** | **いない** | **できない** — PTY に書く主体がもういない |
| `gc_despawned_surfaces` | — | 不要 — 端末ごと消えるので observer は空振りする |
| **mount ゲートでの却下** | 生きているが**知らない** | **できない** |

mount ゲートでの却下がいちばん強い理由である。**APC は一方通行で返信がないので、
クライアントは mount が成功したと思っている。** 未知の instance・非所有 handle・スロット
枯渇・url 解決不能のいずれで host が却下しても伝わらない。一方 VT は `n=` の provenance を
検証できないので、構文さえ合っていれば placement を登録し cap の1枠を消費する。その枠は
アンカー行が履歴リングから落ちるまで解放されない（出力の少ないアプリでは実質永久）。
「誰も unmount を書かない配置」を回収できるのは host だけである。

`unregister` はこの経路に相乗りしているだけで、それ単独では必要条件ではない。ただし
クライアント実装が SDK 経由とは限らない（手書きのプログラムが `unregister` だけ送って PTY に
何も書かない、は普通に起こる）ので、経路がある以上は統一して使う。

**damage は stage しない。** `resize` / `scroll` は `DamageSpan::Full` を stage するが、
placement の削除は**行 damage を一切起こさない** — `apc_dispatch` も
`output.damaged |= …` で liveness を上げるだけである（placement リストは
`FrameTracker::emit` の差分セクションなので、変化しただけでフレーム発行が確定する）。
`resize` と同じ規約で実装すると `unregister` のたびに全ビューポート再描画になる。
戻り値の bool は「本当に消えたか」だけを表し、`OrzmaTty` が coalescer を arm する。

削除処理そのものは `ScreenPlacements` の既存プリミティブ（`evict_where`）に集約する。
id 一本化後、`supersede`・`unmount` のフル指定・host の削除は文字どおり同じ
`retain(|p| p.id != id)` になるので、低層は1本にまとめ、**公開エンドポイントは分けたまま**に
する（interpreter は `OrzmaVt` 全体ではなく分離借用された `DeviceState` / tracker / output を
操作するため、トレイトメソッドを素直に再利用できない）。

**`WebviewEvicted` は出さない。** 呼び出し元の host は既に対象を知っていて ECS 側も
同時に落とすので、signal を返すと自分の despawn を二重に受け取ることになる。
`supersede` が id を名指ししないのと同じ理屈である。

### 2.6 supersede の意味が変わる

現在は「新しい `PlacementId` を採番し、旧 id は名指しされず黙って消える」。一本化後は
同じ `InstanceId` の table エントリを差し替えるだけになり、「観測できないまま消えた id」
という歪みが解消され、kitty の in-place 更新（"without flicker"）に揃う。

## §3 host — コントロールプレーンと `mount.rs`

### 3.1 registry を instance の逆引き付きにする

```rust
pub(crate) struct OrzmaRegistry {
    by_handle:   HashMap<String, OrzmaView>,   // OrzmaView に instances: Vec<InstanceId>
    by_instance: HashMap<InstanceId, String>,  // instance → handle
}

pub(crate) struct RemovedRegistration {
    pub handle: String,
    pub owner_surface: Entity,
    pub instances: Vec<InstanceId>,
}
```

フィールドとメソッドは `pub` にする。可視性の上限は構造体自身の `pub(crate)` で既に
決まっているので、各フィールドに `pub(crate)` を重ねても意味が変わらず、冗長なだけである。

2つの map は同じ `&mut self` メソッドの中でしか触らないので原子的に保たれる。
`remove` / `remove_by_connection` / `remove_by_surface` は `Vec<String>` ではなく
`RemovedRegistration` を返し、呼び出し側は asset の purge・ECS の despawn・VT の
削除を1つの値から駆動する。

採番は `OrzmaRegistry::mint_instance(&mut self, handle: &HandleId) -> Option<InstanceId>` に
置く。**これが `InstanceId` を生む唯一の経路**であり、D3 の不変条件1（allocated な id は
registry 全体で一意）はここで成立する。

**採番方式は `PlacementId` から変わる。** `PlacementId` は端末ごとの単調増加カウンタだったが、
`InstanceId` は 16 バイトの OS CSPRNG である。理由は一意性ではなく**推測不可能性**である —
`PlacementId` は VT から host にしか渡らなかったが、`InstanceId` はクライアントに渡り
PTY 経由で戻ってくる。mount のゲートは `owner_surface`（ペイン単位であって接続単位ではない）
なので、同じペインの別プログラムが id を推測できると他人の配置を mount / unmount できる。
handle が既に 128bit CSPRNG なのと同じ理由である。一意性だけなら host グローバルな
カウンタでも足りる。

`Option` が意味するのは**「未知の handle」だけ**である。乱数の失敗は `mint_id()` と同じく
`.expect()` する（CSPRNG が使えない環境ならアプリは継続できないし、handle 採番だけ panic して
instance 採番だけ fallible という非対称も避ける）。衝突は数千エントリ対 2^128 で到達しないので
`debug_assert!` に置き、`Option` には混ぜない — 到達しない分岐を戻り値に混ぜると、呼び出し側が
書けもテストもできないエラー処理を強いられる。

型を作る部分は `orzma_vt` 側の associated function `InstanceId::from_bytes([u8; 16])` に置き、
乱数は host に残す。`orzma_vt` に `InstanceId::new()`（乱数を引く版）を置かないのは、依存を
増やさないためだけでなく、**D1 で取り除いた「VT 採番」を復活させる引き金を残さないため**である。

### 3.2 live state の権威は ECS のまま

mount 済み webview の live state（どの instance がどの端末のどの slot にいるか）は、
**`Webview` component と `ChildOf` 関係が唯一の権威**であり続ける。`mount()` の重複判定は
現行どおり `live_webview_children` の `Children` スキャン、slot 割り当ては
`smallest_free_slot` のままとする。1端末あたり最大12件なので走査コストは問題にならない。

§3.1 の `by_instance` はこれとは別物で、**allocation と所有権の権威**である
（instance → handle → `owner_surface`）。live かどうかは持たない。

同一 signal バッチ内でこのスキャンが最新の状態を見ることは §5 で保証されている。
派生索引（`TerminalWebviews` のような `HashMap<InstanceId, Mounted>`）は、
ECS state の二重管理になるうえ、その整合性のための不変条件が索引自身の存在によって
生じるハザードを守る循環になるため、採用しない。

### 3.3 mount のゲート列

```text
1. 未知の instance          → registry の逆引きで解決できない  → debug + 回収 + return
2. この端末に同じ instance  → in-place 更新して return
3. resolve_mount(handle)    → 未登録 / owner_surface 不一致    → debug + 回収 + return
4. overlay スロット枯渇                                        → debug + 回収 + return
```

1 を先頭に置くのは、handle が取れないと 3 以降が書けないためである。「回収」は §6 の
VT 側 placement の削除を指す。

url に対応するゲートは無い。url は登録時に検証済み（`invalid_url` /
`unsupported_scheme`）であり、`resolve_mount` は3種の source すべてで必ず url を
組み立てるためである。したがって `ResolvedWebviewMount::url` は `Option<String>`
ではなく `String` である。

### 3.4 コンポーネントの統合

id が1つになると `Webview.instance_id` と `WebviewPlacement.placement` が同じ値になり、
二重持ちになる。1コンポーネントに畳む。

```rust
pub struct Webview {
    pub handle: String,        // view_id からの改名
    pub instance: InstanceId,
    pub slot: u8,
    pub rows: u16,
    pub cols: u16,
}
```

`WebviewPlacement` は削除。`on_placement_removed` は `On<Remove, Webview>` に移り、
compositing 停止通知とマップの刈り取りの両方を担う。`sync_webview_size` は
`(&mut WebviewSize, &Webview)`。`mount` の in-place 更新は `Webview` への `set_if_neq`。

### 3.5 ワイヤの op

追加:

```rust
ClientMsg::NewInstance { handle: String }
ControlEvent::NewInstance { connection_id, owner_surface, handle, reply }
```

listener の dispatch は `Register` と同型（`reply_rx.recv()` でブロックしてから次の行を
読む）ので、接続あたりの応答順序は厳密に保たれる。エラーコードは `unknown_handle` /
`not_owner`。

instance スコープ化するもの:

| op | 現在 | 変更後 | 理由 |
| --- | --- | --- | --- |
| `focus` | `{handle, instance}` | `{instance}` | instance だけで一意に引ける |
| `navigate` | `{handle, action}` | `{instance, action}` | 現在は `.find()` で先頭を選ぶので N インスタンスで曖昧 |
| `compositing`（push） | `{handle, active}` | `{handle, instance, active}` | どの配置が composite したか SDK が判別できない |
| `call`（push） | `{handle, reqId, …}` | `{handle, instance, reqId, …}` | どのページからの呼び出しか区別する |

`WebviewOwner` に `instance: InstanceId` を足し、`call` の push がそれを載せる。

handle スコープのまま残すもの: `emit`（全インスタンスのページに配るのが正しく、
既存の loop がそのまま機能する）、`unregister`、`register`。

### 3.6 D7 の配線

`bevy_orzma_tty` に `RequestTtyWebviewRemove` を新設し、observer が §2.5 の
`OrzmaTty::remove_placements` を叩く（`RequestTtyScroll` と同型）。焚くのは
`Unregister`、`Disconnect`、`gc_despawned_surfaces` の3箇所。いずれも
`RemovedRegistration` から `(owner_surface, instances)` を読む。
`gc_despawned_surfaces` は端末エンティティ自体が消えているので observer が空振りするが、
VT ごと消えているので正しい。

### 3.7 `apply_control_events` の分割

現在 `crates/bevy_orzma_webview/src/control_plane.rs` の 440–650 行（本体 ≈193行）で、
Rust 規約の「system 本体 ~150行」を超えている。`NewInstance` を足すとさらに伸びるので、
各 `ControlEvent` の腕を private helper fn に切り出し、system 本体を振り分けだけにする。

## §4 SDK（`sdk/ratatui_orzma`）

SDK は `orzma_vt` に依存しない独立クレート（`publish = true`）なので、instance は
**不透明な文字列のまま**扱う。host が u128、SDK が String という非対称は意図的である。

### 4.1 型

```rust
pub struct WebviewHandle {
    handle:   Arc<Mutex<HandleId>>, // 登録の id
    instance: Arc<Mutex<String>>,   // 既定 instance
    events:   Arc<EventQueues>,
    writer:   SharedWriter,
    session:  Weak<SessionCore>,    // pending FIFO + registrations
}

pub struct WebviewInstance {
    id:     Arc<Mutex<String>>,
    writer: SharedWriter,
}
```

id スロットが両方 `Arc<Mutex<..>>` なのは、reconnect が中身を差し替えるだけで
クローン済みのハンドルが追随するためである（handle が既に使っている仕組みの踏襲）。

採番は **`WebviewHandle::new_instance(&self) -> OrzmaResult<WebviewInstance>`** に置く。
ハンドルが pending FIFO と `registrations` に到達できないという当初の制約は、その2つを
`SessionCore` にまとめ、ハンドルに `Weak<SessionCore>` を1本持たせることで外した。`Weak`
なのは `SessionCore → Registration → ハンドラのクロージャ → アプリ → WebviewHandle` と
戻る参照循環を構造的に断つためである。`writer` は `SessionCore` に入れずハンドル側に残す
ので、`emit` / `navigate` などの既存メソッドは `Orzma` の生存に依存しない — 失効するのは
採番だけで、`Orzma` を drop した後の `new_instance` は `OrzmaError::SessionClosed` を返す。

本体は `SessionCore::mint_instance` に置き、`WebviewHandle::new_instance` はその委譲に
する。こうすると webview.rs が session.rs から取り込むのは `SessionCore` 一つで済み、
`Pending` / `Registration` / `send_request` を `pub(crate)` に広げずに済む。

**RPC ハンドラの中から呼んではならない。** ハンドラは reader スレッド上で同期実行され、
採番はその同じ reader が応答を捌くのを待つので、ハンドラ内から呼ぶと応答が
タイムアウトするまで停止する。この禁止はメソッドの doc に明記する。

`WebviewHandle::handle_id()` は `HandleId` を、`instance_id()` は既定 instance の
`String` を返す。**無印の `id()` は残さない** — 自然に手が伸びる名前が handle を返すのが
事故の入口だったためで、改名により既存の `view.id()` 4箇所はコンパイルエラーになる。
加えて D8 の newtype に `From<HandleId> for String` が無いので、`handle_id()` の戻り値を
widget に渡しても `impl Into<String>` に適合せず、二重に塞がる。widget と navigate に
渡すのは `instance_id()` である旨を、両メソッドの doc に明記する。

### 4.2 描画モデルを instance キーにする

```rust
struct Placement { instance: String, area: Rect }

struct FramePlacements {
    placements: Vec<Placement>,
    focused: Option<String>,                    // instance
    pending_compositing: HashMap<String, bool>, // instance キー
}

struct FlushState {
    last: HashMap<String, Rect>,                // instance キー
    last_focused: Option<String>,               // instance
}
```

`flush_placements` の畳み込みが instance キーになることで、同一 handle の N 配置が
それぞれ独立して diff される。差分ロジック自体は無変更。

### 4.3 `escape.rs`

```rust
mount(instance, rows, cols) → "\x1b_Omount;n={instance},r={rows},c={cols}\x1b\\"
unmount(instance)           → "\x1b_Ounmount;n={instance}\x1b\\"
```

`valid_handle`（`^[A-Za-z0-9._-]{1,128}$`）は `valid_instance`（`^[0-9a-f]{32}$`）に
置き換える。「不正な値の placement は1件だけスキップして flush 全体を落とさない」防御は
そのまま残すが、**スキップ時に `tracing::debug!` を出す**。現在は無言で捨てており、
D6 で述べた「handle を渡してしまった」ケースが診断不能になるためである。

引数なしの `Ounmount` は SDK からは出さない。ワイヤの綴りとしては残す（手書きの
エスケープハッチ、および RIS / screen flip とは別経路の一括解除として）。

### 4.4 pending FIFO の enum 化

現在の FIFO は `PendingRegister` 型固定で、応答パーサも `handle` 必須を前提にしている。

```rust
enum Pending {
    Register    { reply: Sender<OrzmaResult<(String, String)>>, handlers, events },
    NewInstance { reply: Sender<OrzmaResult<String>> },
}

struct ServerReply {
    ok: bool,
    handle:   Option<String>,
    instance: Option<String>,
    error:    Option<String>,
}
```

どのフィールドが必須かは FIFO の先頭エントリが決める。**pop は検証より先に行う** —
種別が食い違ったときにエントリを残すと FIFO が恒久的にずれるので、必ず1件消費してから
待機側にプロトコル違反エラーを返しログする。

**reqId は導入しない。** 接続あたりの応答順序は host 側で厳密に保たれており
（listener は `register` / `new_instance` で `reply_rx.recv()` をブロックしてから次の行を
読む）、SDK 側も writer lock を保持したまま FIFO に push している。位置対応で十分である。

**現行の判別規則はドキュメントより緩い。** reader は「`call` / `compositing` / `event` の
いずれでもなく、かつ `RegisterReply` としてパースできる行」を応答とみなしており、
`RegisterReply` は `ok: bool` しか必須にしていない（`handle` と `error` は
`#[serde(default)]`、`deny_unknown_fields` なし）。結果として、`"ok": <bool>` を持つ未知の
`op` の行が FIFO を1件消費し得る。仕様書の「op のない行 = 直近リクエストへの応答」は
「最も古い未処理リクエストへの応答」に一般化して書き直すと同時に、**reader 側も
`op` フィールドの不在を明示的に確認する**ようにして、仕様と実装を揃える。

### 4.5 reconnect

```rust
struct Registration {
    kind: RegisterKind,
    handle_slot: Arc<Mutex<HandleId>>,
    instance_slot: Arc<Mutex<String>>,          // 既定 instance
    extra_instances: Vec<Arc<Mutex<String>>>,   // new_instance で作った分
    handlers, events,
}
```

replay ループが registration ごとに、re-register で `handle_slot` と `instance_slot` を
埋め、続けて `extra_instances` の数だけ `new_instance` を発行してスロットを埋める。
`registrations` のロックは**新しいソケットへ接続する前**に取り、replay を抜けるまで保持する。
採番も同じロックを往復越しに握るので、両者は「完全に先行する」か「完全に後続する」かの
どちらかになり、古い handle id の要求が新しい接続に乗ることがない。
既存の `generation.fetch_add(1)` はループを抜けた後にあるので、全スロットが埋まるまで
世代は上がらず、`FlushState::reset()` → 全再 mount の順序は自動的に守られる。

生成した `WebviewInstance` のスロットを `Registration` に登録する経路が要る。`SessionCore`
が `registrations` を持つのはこのためで、`WebviewHandle` はそこへ `Weak` 経由で到達する。

**失敗パスを設計に含める。** 現在の replay ループは、途中の失敗でどこからでも
`disconnected = true` を立てて `return` し、generation を上げず、既に再登録済みの
エントリのロールバックもしない。`new_instance` の往復が registration ごとに N 回増えるので
この露出は増える。加えて応答待ちの `rx.recv()` にはタイムアウトがない。中途半端に
埋まった replay は、`FlushState.last` に古い instance キーを残したまま `reset()` されない
状態を作る。方針:

- replay が途中で失敗したら、そのセッションは切断済みとして扱い、次の reconnect を待つ
  （現行と同じ）。ただし **`FlushState` の無効化は generation ではなく切断フラグでも
  行う**ようにして、古いキーで APC を書き続けないようにする。
- 応答待ちには上限を設ける。

### 4.6 focus と compositing

`flush_focus` は `ClientMsg::Focus { instance: Option<String> }` を送る。reader スレッドは
`compositing` push の `instance` で `pending_compositing` をキーする。
`WebviewWidget::render` は `state.take_compositing(&self.instance)` を引く。
「1フレームに focus を主張する widget は高々1つ」の `debug_assert` はそのまま。

これで「同一 handle の A/B を同時表示し、A だけ unmount したら B まで非 active に
見える」というクロストークが構造的に消える。ただし**別の欠落は残る** — `Orzma::frame()` は
`pending_compositing` を `mem::take` で奪うので、その フレームで描画されなかった
instance の通知は落ちる。これは再キーとは直交する問題で、この変更では扱わない。

**この再キーは原子的な切り替えではない。** host は `instance` を足しても `handle` と
`active` を送り続け、reader は未知フィールドを無視するので、§3.5（host が `instance` を
載せる）だけが先行しても compositing コールバックはそのまま発火する。中間状態で起きるのは
同一 handle の複数配置が1つのマップキーに衝突することだけ — それは §4.6 が解消しようと
している既存欠陥そのものであって、この変更が持ち込む新たな破壊ではない。したがって
§4.6 は「切り替えを壊さないため」ではなく、単に正しいから入れる。

### 4.7 navigate と call の SDK 側

§3.5 で instance スコープにした2つは SDK にも受け皿が要る。

- `navigate` — `ClientMsg::Navigate` に `instance` を載せ、`WebviewInstance` に
  `navigate` / `go_back` / `go_forward` / `reload` を生やす。`WebviewHandle` の同名メソッドは
  **既定 instance を駆動する**（handle 全体へのブロードキャストではない）。
- `call` push の `instance` — `IncomingCall` にフィールドを足す。足さなくても serde が
  黙って捨てるだけでエラーにはならないが、それではハンドラがどのページからの呼び出しか
  永久に判別できず、multi-instance を足す意味が半減する。ハンドラ側 API にどう見せるかは
  この変更のスコープに含める。

## §5 signal バッチ内の逐次可視性

D3 が「unmount 後に同じ id で再 mount してよい」と言う以上、host の webview
ライフサイクルは **signal 順に適用され、各ステップの結果が次のステップから見える**
必要がある。`pump_terminals`（`crates/bevy_orzma_tty/src/signals.rs:143`）は1回の pump の
signal をすべて `commands.trigger` でキューに積むので、この性質が本当に成り立つかは
`bevy_ecs` の command 適用順に依存する。

**成り立つ。** `bevy_ecs-0.19.0` は各コマンドのメタデータに次のトランポリンを
埋め込んでおり（`world/command_queue.rs` の `RawCommandQueue::push`）、**1コマンドごとに
`world.flush()` が走る**:

```rust
command.apply(world);
// The command may have queued up world commands, which we flush here to ensure they are
// also picked up. If the current command queue already the World Command queue, this will
// still behave appropriately because the global cursor is still at the current `stop`,
// ensuring only the newly queued Commands will be applied.
world.flush();
```

したがって `commands.trigger(Evicted{X})` の observer が積んだ despawn は、次の
`commands.trigger(Mount{X})` が走る**前に**適用される。`apply_or_drop_queued` が入口で
`stop = bytes.len()` を固定するのは、この入れ子 flush が「新しく積まれた分だけ」を
適用するための仕掛けであって、適用を妨げるものではない。

実測でも確認済み（production 相当のプローブ）:

```text
[Evicted{X}, Mount{X}] → evict が despawn → mount は live=[] を見て spawn → entity 1つ
[Mount{X}, Mount{X}]   → 1回目 spawn → 2回目は live に見えるので in-place → entity 1つ・slot 1つ
```

**この保証は文書化されていない実装詳細であり、近傍の docs はむしろ逆を述べている。**
`Observer` の docs は "Commands sent by observers are currently not immediately applied.
Instead, all queued observers will run, and then all of the commands from those observers
will be applied" と書くが、これは**同一 trigger の複数 observer**とライフサイクル連鎖
（`OnAdd`→`OnInsert`、bevyengine/bevy#20833）を指すもので、1つのキューに並んだ別々の
`Commands::trigger` コマンド同士には当たらない。

依存が壊れたら静かに壊れる種類のものなので、**この性質を固定する回帰ガードを2本置く**
（§7 のテスト戦略）。既存の `mount.rs` のテストヘルパが `trigger` ごとに
`world_mut().flush()` を挟んでいるのは production を忠実に再現しており、経路が
覆われていないわけではない。ガードは「バージョンアップでこの前提が変わったら落ちる」
ためのものである。

### 検討して採用しなかった対策

いずれも上の保証が成り立たない場合の備えとして検討したが、成り立つ以上は不要である。

- host 側に権威マップ（`TerminalWebviews`）を持ち、observer が `ResMut` で即時更新する
  — observer 内の `ResMut` が即時反映されること自体は正しい（実測で確認）が、ECS の
  派生コピーを増やし、その整合性のための不変条件（entity 一致での刈り取り）が
  マップ自身の存在によって生じるハザードを守るという循環になる。
- `pump()` が signal 列を id ごとに coalesce する — signal が「起きたこと」ではなく
  「正味の結果」になり、`orzma_tty` が VT の placement 意味論を知ることになる。
- ライフサイクルを1つの順序付き適用システムに畳む — 最も原理的だが、signal の面と
  既存テストの書き換えが最大。

## §6 エラー処理と診断

### mount ゲート失敗は VT の幽霊配置も回収する

D7 で host→VT の削除経路ができたので、却下した mount の VT 側 placement を回収できる。

```text
mount() が却下（未知 instance / 非所有 / スロット枯渇 / url 解決不能）
  → tracing::debug!
  → commands.trigger(RequestTtyWebviewRemove { terminal, instances: vec![instance] })
```

現在は、未登録 handle への mount が VT に placement を登録したまま残り、cap の1枠を
アンカーが履歴から落ちるまで占有する。回収を入れると、不正な `n=` を投げ続けても cap を
枯らせない。`WebviewMountRejected`（VT の cap 満杯）は VT が何も登録していないので回収不要。

### プログラムへの通知は入れない

未知 instance の場合そもそも所有者を特定できず、既知だが非所有の場合だけ通知できると
いう非対称が生まれる。既存の未登録 handle と同じ `tracing::debug!` に揃え、push 通知は
将来の別変更とする。

### エラーコード

`new_instance` → `unknown_handle` / `not_owner`。SDK は `OrzmaError` にバリアントを1つ
追加する。malformed な APC は現状どおり黙って捨てる。

## §7 ドキュメント・移行・テスト戦略

### 更新するドキュメント

| 対象 | 内容 |
| --- | --- |
| `docs/orzma_webview_protocol.md` | op 表に `new_instance`、register 応答の形、「op なし行 = 最も古い未処理リクエストへの応答」への一般化、mount / unmount 節の全面書き換え、`instance` の意味論を新設、シーケンス図、例、エラーコード表 |
| `CLAUDE.md` | 「writes an APC `Omount;v=<handle>` sequence」→ `new_instance` を挟む新しい流れ |
| `.claude/skills/enumerate-test-cases/SKILL.md` | `PlacementId` への言及を `InstanceId` に |

### 移行

**破壊的変更で、互換期間は設けない。** `Omount;v=…` は parse しなくなる。根拠は、
`instance_id` が現時点で誰にも使われておらず、リポジトリが `refactor!:` / `fix!` で
破壊的変更を通常運用しているためである。

破壊の内訳は一様ではないので、正確には次のとおり:

| 変更 | 旧クライアントへの影響 |
| --- | --- |
| `Omount;v=…` の廃止 | **破壊的**。古い SDK の mount は malformed として捨てられる |
| `register` 応答への `instance` 追加 | 無害。現行 SDK の `RegisterReply` は未知フィールドを無視する |
| `compositing` push への `instance` 追加 | 無害（読まれないだけ）。§4.6 の再キーと同時である必要はない — 先行しても reader は `handle` で発火し続ける |
| `focus` / `navigate` の instance 化 | **破壊的**。host 側のフィールドが変わる |

SDK と `apps/` 配下の呼び出し元は同じ PR で追随する。

### テスト戦略

§5 の2本は**回帰ガード**である。現在の `bevy_ecs` 0.19 では両方とも green になる
（依存している逐次可視性が文書化されていない実装詳細なので、バージョンアップで壊れたら
落ちるようにしておく）。production 相当の形にするには、`world.trigger()` を直接複数回
呼ぶのではなく、**1つの system から複数の `commands.trigger` を積み、deferred の適用を
1回走らせる**必要がある。既存ヘルパの per-trigger `flush()` は production を忠実に
再現しているので、そちらはそのまま残す。

**先行して必要な harness 変更がある。** SDK の `FakeServer::start`
（`sdk/ratatui_orzma/tests/support/mod.rs`）は `{"ok":true,"handle":…}` を自動応答し
`instance` を返さない。また `next_message()` は `hello` / `register` しか除外しないので、
`new_instance` の行が後続のアサーションに漏れ込む。両方を先に直す。

| 層 | 追加するテスト |
| --- | --- |
| `orzma_vt` | `InstanceId::from_str` の境界（31/32/33桁・大文字・`+` 接頭辞・非16進）／`remove_placements` が両スクリーンを走査し短絡しない・**行 damage を stage しない**（`resize` と違う点なので明示的に固定する）・消えたときだけ `true` を返す／unmount のキーが順序独立 |
| `orzma_tty` | `remove_placements` が本当に消えたときだけ coalescer を arm する |
| `bevy_orzma_webview` | **回帰ガード**: `[Evicted{X}, Mount{X}]` を production 形状の1バッチで処理して webview が1つ残る／`[Mount{X}, Mount{X}]` で entity が1つ・スロットが1つ。ほかに、未知 instance の mount が何も spawn せず VT 側も回収される／`new_instance` の handle ゲート／`unregister` が ECS と VT の両方を落とす（既存バグの回帰）／同一 handle の2 instance で `compositing{active:false}` が片方だけを名指しする |
| `sdk/ratatui_orzma` | 同一 handle の2 instance が1フレームで2本の mount になる／片方だけ area 変更で片方だけ再 mount／`Pending` の enum が混在 FIFO で正しい待機者に届く（種別不一致でも FIFO がずれない）／reconnect 後に既定＋追加 instance の両スロットが埋まる／不正な instance のスキップが `tracing::debug!` を出す |

既存テストで書き換えが要るもの（網羅ではない）: `escape.rs::rejects_out_of_range_dims`
（handle `"h"` を使っており、charset ゲートで先に落ちて寸法範囲を検査しなくなる）、
`flush_skips_degenerate_area`（誤った理由で通るようになる）、`WebviewHandle::new_shared` の
3引数呼び出し2箇所、`interpreter/apc.rs` の unmount 順序テスト
（`unmount_instance_without_view_rejected` は消え、代わりに順序独立を固定する）。

`bevy_orzma_webview` のテストは CEF の provisioning（`just setup-cef`）が前提である。

## 却下した案

**A. placement id をクライアントが採番する（kitty の実モデル）** — 当初は「VT は
`PlacementId` の非再利用性を守るため内部 id を持ち続ける必要があり、識別子は結局2つ
残る」として却下した。**この根拠は失効している** — D3 が不変条件を「同時に live な id の
一意性」に弱めた時点で、内部 id は不要になった。したがって案 A は再評価が必要になり、
その結果が次の A' である。

**A'. `v=<handle>` を残し `n=` をクライアントが採番する（`(handle, instance)` の組を
アドレスにする、通称「設計2」）** — 再評価の結果、**技術的には D1 より多くの点で優れて
いた**。記録として差分を残す。

- 往復ゼロ（`new_instance` op が不要）、reconnect の replay は 1 回、`WebviewWidget::new`
  の既存16箇所が無変更、registry の逆引きも `ServerMsg` の分割も不要。
- D7 は既存の `DeviceState::unmount_placement(Some(handle), None)` をそのまま再利用でき、
  VT 側の新規テーブル処理はゼロ。
- 衝突は構造的にゼロ（アドレスの先頭が host 採番の 128bit handle なので、別プログラムが
  同じ `n=1` を選んでも組は異なる）。同一 handle 内の重複は自分の名前重複＝supersede で、
  kitty が "without flicker" と呼ぶ意図された挙動。
- 先行事例は圧倒的にこちら（kitty 仕様、WezTerm 実装、X11、Wayland、HTTP/2、AIP-133）。
- 唯一の技術的代償は `AnchoredPlacement` が `Copy` を失うこと（64B）。実測では
  `project() + diff_placements` が 1 emit あたり 28-49ns → 410-487ns だが、coalescer が
  emit を約 333 回/秒に制限するため差は **約 140µs/秒 ≒ 1コアの 0.014%**
  （レンダラ込みで 0.03%、webview 0 個ならゼロ）。決定的な反対理由にはならない。
  なお `n=` を `1..=4294967295` に制限し handle を u128 にデコードすれば 32B で `Copy` を
  維持できる（設計2b）。

**それでも D1 を採る理由は1つ** — 設計2 では `orzma_vt` が `view_id: String` を持ち
続ける（中身を解釈することは一度もない不透明な文字列として）。**VT から handle 概念を
排除することを最上位要件とする**判断により、上記のコストを承知で D1 を採用した。

**B. VT が採番して PTY 返信で返す** — クライアントが端末返信をパースする必要があり
（状態機械＋タイムアウト）、mount が request/response になる。さらに TTY は全チャンクを
drain して reply を書いた後で signal を Bevy に返すので、GUI mount 完了の acknowledgment
にはならない。reply 単独では damage も arm しない。先行事例もゼロ。

**D. multi-placement 自体をやめる**（1ハンドル＝1配置、N 個欲しければ N ハンドル登録）—
現時点で `instance_id` が未使用なので回帰ゼロ、プロトコルは最小になる。ただし同一論理
ビューの N 配置が N オリジンになり、localStorage 等を共有できない。「同一ハンドルの
複数配置は必要」という判断により却下。

**E. `handle` を `view_id` に改名する** — 「handle は OS/API の語彙では1つの生きた
リソースへの参照を指すので、定義側を指す今の用法は直感と逆」という指摘から検討した。
却下した理由:

- **handle は実際には正確である。** orzma の handle は静的なクラス定義ではなく、
  `register` で生成され `unregister` / 切断 / surface 消滅で失効する、接続に所有された
  生存期間付きリソースへの参照である。X11 の resource ID（生成済みの window / pixmap /
  font を指す）と DRM GEM の handle（file-local な参照で破棄時に参照を落とす）が同じ形。
- **`view` は逆向きの曖昧さを持ち込む。** Wayland の `wl_surface`（画面上の矩形）が
  示すように、ウィンドウシステムの語彙では「画面に出るもの」側に寄る。「1サイトと
  複数タブ」の比喩で、`view` はサイトにもタブにも自然に読めるため、候補の中で最も
  識別力が低い。
- **影響範囲が改善に見合わない。** 単純全文検索で 591 hit / 24 files、ソケット JSON の
  6〜7 メッセージ形、`orzma://<handle>/` の URL・scheme parser・`WebviewAssetRegistry`
  の API、そして `publish = true` の SDK の公開型 `WebviewHandle`。
- **そもそも名前は事故の原因ではなかった。** 実際の事故経路は、`WebviewHandle` が
  `id()` と `instance_id()` を同居させていることである。D8 の newtype がそこを直接塞ぐ。

### レビューで検討し、採用しなかったもの

- **`uuid::Uuid` を `InstanceId` に使う**（`.simple()` がちょうど32桁小文字 hex を出す）—
  v4 は6ビットを version / variant に使うので乱数が 122bit に減り、`Uuid::try_parse` は
  ハイフン形・波括弧形も受けるので結局厳格形の検査が要る。なにより `orzma_vt` に
  公開ワイヤ形式の依存が増え、D4 の採用条件（`orzma_vt` に依存を足さない）に反する。
- **`ServerMsg` を1バリアント（`handle: Option<String>`）にまとめる** — SDK が既に
  フラットな構造体でパースしているので §4.4 の差分は減る。ただし「register の成功応答には
  必ず handle がある」を型で表現できなくなるため、D5 のバリアント分割を維持する。
- **`WebviewWidget::new` を型付きにする**（`&WebviewInstance` か newtype）— D6 の5箇所の
  無言破壊をコンパイルエラーに変えられる。設計判断としてトレイトを入れないと決めたので、
  代わりに全呼び出し元の書き換えと `tracing::debug!` を必須作業に含めた（D6 参照）。
- **素の `Ounmount`（全配置解除）も落とす** — パース分岐が1つ減り、`WebviewUnmount` が
  `Option<InstanceId>` ではなく `InstanceId` になる。手書きのエスケープハッチとして残す。
- **`getrandom` を 0.2 から 0.3 / 0.4 に上げる** — orzma の crate 群が 0.2 の唯一の消費者で、
  0.3 / 0.4 は既に lock にいるため重複メジャーバージョンを1つ減らせる。この変更の
  スコープ外とし、別途行う。

## 出典

- kitty graphics protocol — <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
  （引用は rst 原文。"Requesting image ids from the terminal" / "Deleting images" /
  "Display images on screen" の各節）
- Ghostty `src/terminal/kitty/graphics_storage.zig`（`next_internal_placement_id` の
  "This is never user-facing" コメント、`PlacementId` の internal/external タグ）
- WezTerm `term/src/terminalstate/kitty.rs`
- iTerm2 Inline Images Protocol — <https://iterm2.com/documentation-images.html>
  （識別子パラメータを持たない）
- DEC sixel（VT330/VT340 Programmer Reference, Chapter 14）— DCS パラメータは
  P1/P2/P3 のみで識別子なし
