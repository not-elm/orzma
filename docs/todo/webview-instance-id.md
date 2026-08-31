# webview 配置識別子を host 採番の `InstanceId` に一本化する

`PlacementId` を削除し、コントロールソケットが払い出す `InstanceId` 1本に統合する。
未着手。調査と設計判断まで完了した段階の記録。

前提となる作業（APC ディスパッチの実装、SDK の APC 移植、仕様書の同期）は
PR #269 で完了している。この文書はその次の段階を扱う。

## 現状: 識別子が3層ある

| 識別子 | 誰が採番 | いつ | クライアントに返るか |
| --- | --- | --- | --- |
| `view_id`（handle） | **host**（コントロールプレーン、128bit 乱数） | `register` 時 | **返る**（コントロールソケット） |
| `instance_id` | **クライアント** | mount を書く前（任意） | 返らない（自分で決めたので不要） |
| `PlacementId` | **VT** | mount を処理した後 | **返らない**（host にのみ渡る） |

`(view_id, instance_id)` がクライアントから見たアドレスで、`PlacementId` は
VT ↔ host 間の内部アドレス。フレームが運ぶ `AnchoredPlacement` も、
`WebviewEvicted` が名指しするのも `PlacementId` のほう。

`instance_id` は**現時点で誰も使っていない**。SDK の `mount()` に instance
引数がなく、`Session` は focus 操作に常に `instance: None` を送る。`apps/`
配下にも使用箇所はない。

## 何が問題か — 採番の方向が逆

`instance_id` と `PlacementId` は名前が違うだけの2つの id ではない。
**誰がいつ決めるか**が逆になっている。

- `instance_id` は**クライアントが mount を書く前**に決めるので、後の
  `unmount` で自分で指名できる。
- `PlacementId` は**VT が mount を処理した後**に採番する。APC は一方通行
  （プログラムが PTY に書くだけ）なので、クライアントは自分の placement の
  id を知る手段がない。

したがって「`PlacementId` だけにして `unmount` もそれで指名する」は、
現行プロトコルのままでは成立しない。`instance_id` が存在する理由はここにある。

## 先行事例調査

kitty の仕様原文（rst ソース）と kitty / Ghostty / WezTerm の実装ソースを
突き合わせた結果。

**kitty の `p=`（placement id）はグローバルではない。** 仕様が明記している —
"Every placement is uniquely identified by the **pair** of the `image id` and
the `placement id`." つまり per-image スコープで、`p=7` は image 10 と image 11
で別物。削除も必ず image id 起点（`d=i,i=10,p=7`）で、**`p=` 単独でアドレスする
削除は kitty に存在しない**。

**「端末が採番してクライアントに返す」経路は kitty に実在するが、返るのは
image id であって placement id ではない。** `I=`（image number）を送ると端末が
`i=`（image id）を採番して `<ESC>_Gi=99,I=13;OK<ESC>\` で返す。これは orzma の
`register → {ok, handle}` と**構造的に同型**で、違いは kitty が帯域内、orzma が
帯域外（Unix ソケット）という点だけ。

**グローバルなクライアント採番 id は kitty 自身が失敗と認めている。** "Since
IDs are in a global namespace **there can easily be collisions**" と書かれ、
`I=` はこの衝突を回避するために後から足された間接層。Ghostty はさらに、
暗黙採番を `2147483647` から始めて「クライアントが選びがちな低域を避ける」と
コメントしている。orzma は handle を host 採番の 128bit 乱数にしたことで、
この問題を最初から消している。

| | 端末が placement id を採番するか | wire に出るか |
| --- | --- | --- |
| kitty | しない | — |
| Ghostty | する（`next_internal_placement_id`） | **出ない**（"never user-facing" と明記） |
| WezTerm | しない | — |
| iTerm2 / Sixel | placement という概念自体がない | — |

**「端末が採番したグローバル一意な placement id を唯一の wire アドレスにして
いる実装」は見つからなかった。** orzma の `PlacementId` は Ghostty の internal
placement id と同型で、その設計自体には先行事例がある。

## 決定

**host（コントロールプレーン）が `InstanceId` を採番し、mount より前に
コントロールソケットで払い出す。**

```
client → socket:  new_instance { handle }
socket → client:  { ok, instance: "<128bit base32>" }
client → PTY:     ESC _ Omount;v=<handle>,n=<instance>,r=<n>,c=<n> ESC \
client → PTY:     ESC _ Ounmount;n=<instance> ESC \      ← view_id 不要
```

`PlacementId` は削除し、`InstanceId` が VT の保持する id・フレームが運ぶ id・
ワイヤのアドレスを兼ねる。名前は `InstanceId` だが、スコープはグローバル
（実態は placement id）。

採用理由:

- **orzma が既に採用しているパターンの再適用**。`register → handle` と同じ形で、
  新しい機構を導入しない。kitty がこれを placement に対してできなかったのは
  側チャネルを持たないからで、orzma にはある。
- **mount が fire-and-forget のまま**。id はクライアントが既に持っているので、
  「VT が採番して後から返す」案に伴う非同期性・対応付けの曖昧さ・失敗通知の
  設計が丸ごと不要になる。
- 衝突は host 採番の 128bit 乱数で構造的に消える。
- SDK は既存の pending-reply 機構をそのまま使える。

## 却下した案

**A. placement id をクライアントが採番する（kitty の実モデル）** — 実質
「`instance_id` を改名して必須化する」だけ。VT は `PlacementId` の非再利用性を
守るため内部 id を持ち続ける必要があり、**識別子は結局2つ残る**。

**B. VT が採番して PTY 返信で返す** — クライアントが端末返信をパースする必要が
あり（状態機械＋タイムアウト）、mount が request/response になる。さらに TTY は
全チャンクを drain して reply を書いた**後で** signal を Bevy に返すので、
**GUI mount 完了の acknowledgment にはならない**。reply 単独では damage も
arm しない。先行事例もゼロ。

**D. multi-placement 自体をやめる**（1ハンドル＝1配置、N 個欲しければ N ハンドル
登録）— 現時点で `instance_id` が未使用なので回帰ゼロ、プロトコルは最小になる。
ただし**同一論理ビューの N 配置が N オリジンになり、localStorage 等を共有できない**。
「同一ハンドルの複数配置は必要」という判断により却下。

## 壊れるもの — `PlacementId` の非再利用に依存している箇所

`PlacementId` の doc は「単調増加・セッション中再利用なし」を不変条件として
宣言している（`crates/orzma_vt/src/placement.rs:10-18`）。調査の結果、
**単調な大小関係に依存する production コードは存在しない**。実際に必要なのは
非再利用性だけで、これは host の 128bit 乱数採番で満たせる（tombstone 不要）。

再利用が起きた場合に壊れる箇所:

| 現象 | 箇所 |
| --- | --- |
| 遅延した `WebviewEvicted{X}` が id だけ見て despawn し、X を再利用した後継が破棄される | `crates/bevy_orzma_webview/src/webview/mount.rs:463,474` |
| その誤 despawn が `on_placement_removed` を焚き、live なはずのビューに `compositing{active:false}` を送る | `mount.rs:647` |
| 同一 id の live placement が複数あると、投影の `.find()` が最初の geometry を両方に割り当てる | `mount.rs:600` |

signal とフレームの到着順は入れ替わり得ることがテストで固定されているため、
「同時に一意」では不十分で、契約は**セッション全期間で再利用しない**である必要がある。

## 積み残しの手当て

**1. VT は id を検証できない。** `orzma_vt` はコントロールプレーンを知らないため、
ワイヤ上の `InstanceId` が正当に払い出されたものか判断できない。検証は host 側に
移り、`mount()` の既存ゲート列（未登録ビュー、所有権不一致、オーバーレイスロット
枯渇）に「未知の instance id の棄却」が1つ増える。所有権チェックは既に
コントロールプレーンにあるので新概念ではない。

**2. SDK の描画モデルを作り直す必要がある。** `flush_placements` は全 placement を
`HashMap<String, Rect>`（handle をキー）に畳んでいる（`sdk/ratatui_orzma/src/session.rs:361`）。
**同一ハンドルの複数配置が構造的に不可能**なので、multi-placement を使うにはここを
`InstanceId` キーに変える必要がある。どの案を採っても必要な作業。

**3. `PushMsg` は現在 `Compositing` の1バリアントのみ**
（`crates/bevy_orzma_webview/src/control_plane/protocol.rs:187-196`）。
`new_instance` は push ではなく request/reply なので、`ServerMsg`
（`protocol.rs:150-155`、現在 `#[serde(untagged)]` で `Ok { ok, handle }` /
`Err { ok, error }` の2形）にバリアントを足すことになる。**untagged なので
デシリアライズ順序に依存する** — SDK 側で `handle` と `instance` を持つ2つの
成功応答が区別できるか、フィールド名で判別できる形にするか要検討。

**4. supersede の挙動が kitty と違う。** kitty は同じ `(i,p)` への再 put を
**id 不変の in-place 更新**にする（"without flicker"）。orzma は現在 supersede で
**新しい `PlacementId` を採番**し、旧 id は `WebviewEvicted` に名指しされず
黙って消える。`InstanceId` 一本化後は同じ `InstanceId` への再 mount が自然に
in-place 更新になり、kitty に揃うと同時に「消えた id が観測できない」歪みも解消する。

## 未解決の論点

- `unmount` を `n=` 単独で受けるとき、`view_id` は完全に不要にするのか、
  互換のため `v=` 単独（全インスタンス）も残すのか。
- クライアントが払い出しを受けずに `n=` を自作した場合の扱い。host のゲートで
  落とすとして、ログだけか、`WebviewMountRejected` 相当を返すか。
- `InstanceId` の型。`String`（handle と同じ base32）にするか、VT 内では
  数値に落とすか。フレームが運ぶ以上、比較コストが効く。

## 作業中に見つかった別件（この PR で修正済み）

`crates/bevy_orzma_webview/src/control_plane/protocol.rs:153` の doc コメントが
まだ `OSC mount;<handle>` と書いていた。PR #269 の一掃 grep
（`5379|OSC-based|webview OSC|mount OSC|unmount OSC`）は `OSC mount;` という
語順を拾わないため見逃していた。この文書と同じ PR で `APC Omount;v=<handle>` に
直した。

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
