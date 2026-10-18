# Test cases: sync_ime_candidate_area

状態: 承認済み

`src/input/ime.rs:239` の `sync_ime_candidate_area` について、テストケースを列挙した。根拠は2つある。1つはこのメソッドの doc comment(`ime.rs:221-238`)で、種別2として7件を登録した。もう1つは X11 の一文が指す `xlib.pdf` の XNSpotLocation の記述で、3件を登録した。引用は 10/10 件を確認済み。

結果はケース3件で、すべて種別2だけに基づく TC-A であり、すべて要確認。API 改定の提案は1件ある。ほかに、どのシグネチャでもテストから観測できない契約が3件、doc comment とマニュアルの矛盾が1件ある。

作者が 2026-10-18 にこの一覧を承認した。

一覧は仕様と doc comment から導き、自分で検証したもので、作者の承認のほかにレビューは受けていない。下の Rust はコードに書き起こし済みで、今の木に対してコンパイルできる。本体が `todo!()` なので、3件とも失敗する。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-A1 | `an_empty_candidate_area_does_nothing` | NN | High | 要確認 |
| TC-A2 | `a_candidate_area_without_a_primary_window_does_nothing` | NN | High | 要確認 |
| TC-A3 | `a_primary_window_without_a_winit_window_does_nothing` | NN | High | 要確認 |

Source タグ — **NN**: `ime.rs:231-232`「Nothing happens when `ImeCandidateArea` holds `None`, when there is no primary window, or when no winit window backs the primary window.」

3件とも、doc comment が「何もしない」と述べた3つの条件を1つずつ確かめる。マニュアル由来のケースはない。xlib の3件は、テストから観測できないか、doc comment と矛盾するかのどちらかだからである(付録の「観測できない契約」と「仕様の矛盾」)。

テストコードは `src/input/ime.rs` の `mod tests::sync_ime_candidate_area` に置く。作者の指示で、`mod tests` はテスト対象ごとのサブモジュールに分けてある。下の「共通のヘルパ」も同じサブモジュールに置く。

## 共通のヘルパ

```rust
/// The terminal's cursor cell at (70, 130) in logical pixels, one 4 × 8 cell
/// large.
const CURSOR_CELL: Rect = Rect {
    min: Vec2::new(70.0, 130.0),
    max: Vec2::new(74.0, 138.0),
};

/// An app holding `area` as the candidate area and no primary window.
fn sync_app(area: Option<Rect>) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(ImeCandidateArea(area));
    app
}

/// Spawns a primary window that no winit window backs.
fn spawn_primary_window(app: &mut App) {
    app.world_mut().spawn((Window::default(), PrimaryWindow));
}
```

`MinimalPlugins` には `WinitPlugin` が含まれないので、`WINIT_WINDOWS` は空のままである。だから `spawn_primary_window` が作るウィンドウを裏付ける winit のウィンドウは、どのテストにもない。サブモジュールでは `RunSystemOnce` と `MinimalPlugins` を `use` で取り込む。`Rect`、`Vec2`、`Window`、`PrimaryWindow` は `use super::*;` で使える。

## TC-A1 — 候補の領域が `None` なら何もしない

| | |
| - | - |
| Setup | `sync_app(None)`; `spawn_primary_window` |
| Act | `run_system_once(sync_ime_candidate_area)` |
| Expect | panic せずに終わり、`Ok` を返す **[NN]** |

タブ名の変更に入ると、ポリシーは候補の領域を `None` にする。このシステムはその変化で実行される。このケースはプライマリウィンドウを置き、領域だけを空にする。

ただし単体テストには winit のウィンドウがないので、何もしなかった理由が `None` なのか、winit のウィンドウがないからなのかは区別できない。このケースが捕まえるのは、`None` のときに `expect` などで panic する実装である。

```rust
/// Asserts that the sync does nothing when the candidate area holds no
/// rectangle.
///
/// Case: the user starts renaming a tab, so the candidate area that held the
/// terminal's cursor cell is cleared.
#[test]
fn an_empty_candidate_area_does_nothing() {
    let mut app = sync_app(None);
    spawn_primary_window(&mut app);

    let result = app.world_mut().run_system_once(sync_ime_candidate_area);

    assert!(result.is_ok());
}
```

## TC-A2 — プライマリウィンドウがなければ何もしない

| | |
| - | - |
| Setup | `sync_app(Some(CURSOR_CELL))`。プライマリウィンドウは置かない |
| Act | `run_system_once(sync_ime_candidate_area)` |
| Expect | panic せずに終わり、`Ok` を返す **[NN]** |

領域は `Some` にして、`None` の条件では終わらないようにしている。そのうえでプライマリウィンドウを置かない。このケースが捕まえるのは、`primary_window.single()` の失敗を `expect` などで panic にする実装である。

```rust
/// Asserts that the sync does nothing when there is no primary window.
///
/// Case: the primary window has been closed while the candidate area still
/// holds the terminal's cursor cell.
#[test]
fn a_candidate_area_without_a_primary_window_does_nothing() {
    let mut app = sync_app(Some(CURSOR_CELL));

    let result = app.world_mut().run_system_once(sync_ime_candidate_area);

    assert!(result.is_ok());
}
```

## TC-A3 — プライマリウィンドウを裏付ける winit のウィンドウがなければ何もしない

| | |
| - | - |
| Setup | `sync_app(Some(CURSOR_CELL))`; `spawn_primary_window` |
| Act | `run_system_once(sync_ime_candidate_area)` |
| Expect | panic せずに終わり、`Ok` を返す **[NN]** |

領域を `Some` にし、プライマリウィンドウも置く。こうすると、NN の3つの条件のうち winit のウィンドウがないことだけが残る。このケースが捕まえるのは、`WINIT_WINDOWS` からの取得結果を `expect` などで panic にする実装である。

```rust
/// Asserts that the sync does nothing when no winit window backs the primary
/// window.
///
/// Case: the primary window entity has been spawned, but winit has not yet
/// created the native window behind it.
#[test]
fn a_primary_window_without_a_winit_window_does_nothing() {
    let mut app = sync_app(Some(CURSOR_CELL));
    spawn_primary_window(&mut app);

    let result = app.world_mut().run_system_once(sync_ime_candidate_area);

    assert!(result.is_ok());
}
```

## 付録

### 契約表

マニュアル由来のエントリ(3件とも verify 済み):

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | XNSpotLocation | unconditional | "The XNSpotLocation argument specifies to the input method the coordinates of the spot to be used by an input method executing with XNInputStyle set to XIMPreeditPosition." | `xlib.pdf` p.320, L18603-18605 |
| C2 | XNSpotLocation | unconditional | "The x coordinate specifies the position where the next character would be inserted." | `xlib.pdf` p.320, L18608 |
| C3 | XNSpotLocation | unconditional | "The y coordinate is the position of the baseline used by the current text line in the focus window." | `xlib.pdf` p.320, L18609-18610 |

種別2の台帳(7件とも `grep -F` で確認済み。`///` を外して1行にしたものと照合した):

| タグ | 場所 | 引用 | 根拠にするもの |
| - | - | - | - |
| IM | `src/input/ime.rs:221-222` | "Tells the primary window's IME which rectangle its candidate window must not cover." | OS の IME に、覆ってはいけない矩形を伝える |
| LP | `src/input/ime.rs:224-225` | "The rectangle is in logical pixels of the window's client area, with the origin at the top-left corner and the y axis pointing down." | 矩形の単位と座標系 |
| RP | `src/input/ime.rs:225-226` | "It replaces the 10 × 10 area bevy_winit derives from `Window::ime_position`." | bevy_winit の 10 × 10 の領域を置き換える |
| CO | `src/input/ime.rs:226-229` | "While `ImeCandidateArea` holds `Some`, `ime_position` must change only together with it: if `ime_position` changes alone, bevy_winit overwrites the rectangle with its 10 × 10 area and the rectangle is not restored." | 呼び出し側が守る条件 |
| NN | `src/input/ime.rs:231-232` | "Nothing happens when `ImeCandidateArea` holds `None`, when there is no primary window, or when no winit window backs the primary window." | 何もしない3つの条件 |
| OR | `src/input/ime.rs:234-235` | "Must run after bevy_winit hands `ime_position` to winit in the same frame; running first lets the 10 × 10 area overwrite the rectangle." | 実行順の前提条件 |
| XS | `src/input/ime.rs:237-238` | "On X11 the size is ignored, and only the origin reaches the input method as its spot." | X11 の制限 |

### 探索した語

| 段 | 出どころ | 語 | 結果 |
| - | - | - | - |
| 1 | メソッドの `///` | IME | 単語としては全マニュアルで0件 |
| 1 | 同上 | candidate window | 全マニュアルで0件 |
| 1 | 同上 | rectangle | VT510 31件、xterm-ctlseqs 22件、Xlib 199件。どれも矩形領域の操作や描画の話で、このメソッドを規定しない。除外 |
| 1 | 同上 | logical pixel | VT510 の1件(L13799、DRCS のピクセルパターン)だけ。除外 |
| 1 | 同上 | client area | 全マニュアルで0件 |
| 1 | 同上 | primary window | 全マニュアルで0件 |
| 1 | 同上 | winit / 10 x 10 | 全マニュアルで0件 |
| 1 | 同上 | X11 | xterm-ctlseqs 11件(版の名前や歴史の話)。除外。Xlib 113件は次の spot で絞り込んだ |
| 1 | 同上 | input method / spot | Xlib 314件 / 35件。L18601 の「Spot Location」の節が XS の述べる spot を定義している → C1、C2、C3 |
| 2 | 囲む型の doc | (なし) | 自由関数。引数の `ImeCandidateArea` にも doc comment がない |
| 3 | `src/input/ime.rs` の `//!` | IME / composition / keyboard focus | 前2回の実行と同じ結果 |
| 4 | `ImeCandidateArea` → | XNArea / preedit area | Xlib 18件 / 11件。p.319 L18563-18568 は、XIMPreeditPosition でプリエディットを描く範囲(クリッピング領域)の話で、候補ウィンドウに避けさせる矩形ではない。除外 |
| 4 | `PrimaryWindow` → | focus window | Xlib 38件。XIM の XNFocusWindow のことで、C3 の「focus window」と同じ。新しいエントリはない |

### 仕様にないもの

- **ログを出すかどうか。** NN は「何もしない」と述べるが、winit のウィンドウが見つからないときに警告を出すかどうかは書かれていない。
- **拡大率が変わったときの振る舞い。** ウィンドウを DPI の違うモニタへ移すと、論理 px の矩形は変わらないことがある。そのとき OS に渡した矩形を物理 px で渡し直すかどうかは書かれていない。
- **`ImeCandidateArea` そのものの契約。** 型に doc comment がなく、中身の意味はこのメソッドの doc comment にしか書かれていない。

### 仕様の矛盾

- **XS と C3。**
  - XS(`ime.rs:237-238`)は、X11 では矩形の起点だけが入力メソッドの spot になると述べる。LP によれば、起点は矩形の左上、つまりカーソルのセルの上端である。
  - C3(`xlib.pdf` p.320, L18609-18610)は、spot の y を現在の行のベースラインと定めている。
  - 2つを合わせると、X11 ではセルの上端がベースラインとして入力メソッドに渡ることになる。x については、C2 の「次の文字が挿入される位置」とセルの左端が一致するので矛盾しない。
  - ケースは作っていない。X11 をどう扱うかは要件の未決事項と同じ問題である。

### API 改定

提案は1件で、1つの変更として扱う。

#### 提案 1 — 矩形を winit の引数に変える部分を `ImeCandidateArea` のメソッドにする

- **根拠のエントリ:** LP(`ime.rs:224-225`)
- **書けない行:** 「winit の `set_ime_cursor_area` が `LogicalPosition::new(70.0, 130.0)` と `LogicalSize::new(4.0, 8.0)` を受け取る」
- **種別:** O。システムは winit を呼ぶだけで、ECS の状態を何も変えない。しかも単体テストでは `WINIT_WINDOWS` に winit のウィンドウを置けないので、Setup も作れない。
- **シグネチャ:**

  ```rust
  impl ImeCandidateArea {
      fn cursor_area(&self) -> Option<(LogicalPosition<f32>, LogicalSize<f32>)>
  }
  ```

- **メソッドの外に及ぶ変更:** `ime.rs` に `use winit::dpi::{LogicalPosition, LogicalSize};` を足す。winit は `Cargo.toml:46` で直接の依存になっている。`sync_ime_candidate_area` は、このメソッドの戻り値をそのまま `set_ime_cursor_area` に渡す。

現状のケース(書けない):

| | |
| - | - |
| Setup | プライマリウィンドウと、それを裏付ける winit のウィンドウ; `ImeCandidateArea(Some(CURSOR_CELL))` — **書けない** |
| Act | `run_system_once(sync_ime_candidate_area)` |
| Expect | winit が `LogicalPosition::new(70.0, 130.0)` と `LogicalSize::new(4.0, 8.0)` を受け取る **[LP]** — **書けない** |

提案を採った場合のケース(TC 番号も Rust もまだ付けない):

| | P-1 | P-2 |
| - | - | - |
| Setup | `ImeCandidateArea(Some(CURSOR_CELL))` | `ImeCandidateArea(None)` |
| Act | `area.cursor_area()` | `area.cursor_area()` |
| Expect | `Some((LogicalPosition::new(70.0, 130.0), LogicalSize::new(4.0, 8.0)))` を返す **[LP]** | `None` を返す **[NN]** |

P-2 は、NN の「`None` なら何もしない」を、提案の下では「winit に渡すものがない」と読んだものである。

- **採らない場合:** LP からはケースが出ない。
- **採った場合の限界:** 確かめられるのは変換だけである。システムがその値を本当に winit に渡すことは、テストからは確かめられない。

### 観測できない契約

どのエントリも仕様としてははっきりしていて、引用も確認済みである。しかし OS の IME や winit の内部の状態は、どのシグネチャを変えてもテストから見えない。そのため提案はなく、ケースもない。各 OS での手動確認でしか確かめられない。

- **IM**(`ime.rs:221-222`): 書けない行は「OS の IME が、カーソルのセルを覆わない位置に候補ウィンドウを出す」。
- **RP**(`ime.rs:225-226`): 書けない行は「winit が最後に受け取った領域が、bevy_winit の 10 × 10 ではなくこの矩形である」。
- **XS と C1、C2**(`ime.rs:237-238`、`xlib.pdf` p.320 L18603-18605, L18608): 書けない行は「X11 では、矩形の起点が XNSpotLocation として入力メソッドに渡り、x は挿入位置になる」。

### ほかの項目が負う契約

このメソッドの doc comment にあるが、守るのはほかの項目なので、このメソッドのケースにはしていない。

- **CO** は `ime_policy_system` が守る条件である。`ime_policy_system` のケースとして、`ImeCandidateArea` と `ime_position` を同じ実行で変えることを確かめられる。今の `ime_policy_system` の一覧は、`ImeCandidateArea` の中身を「仕様にないもの」にしている。
- **OR** は `ImePlugin::build` の登録が守る条件である。bevy_winit の `changed_windows` は公開されておらず SystemSet もないので、確かめられるのは `MainScheduleOrder` で `ImeCandidateSync` が `Last` の後にあることまでである。

### docs/todo との衝突

`docs/todo/tdd-ime-ime_policy_system.md` は、「仕様にないもの」に `ImeCandidateArea` の中身を挙げている。この文書の CO と LP が、その一部を doc comment にしたものである。食い違いではないが、`ime_policy_system` の一覧はこの doc comment より前に作ったものである。
