# Test cases: ime_policy_system

状態: 承認済み

`src/input/ime.rs` の `ime_policy_system(area: ResMut<ImeCandidateArea>, primary_window: Query<&mut Window, With<PrimaryWindow>>, params: ImePolicyParams)` が負うテストケース。`docs/references/` のマニュアルに `ime_policy_system` を規定する記述はなかったため、すべてのケースはリポジトリ内の doc comment(種別2)を根拠にした `TC-A` である。仕様(マニュアル)由来のケースは0件。

- 引用の検証: **15/15**(リポジトリの doc comment 13件を `grep -F` で、付録で参照する `xlib.pdf` の2件を `verify` で確認)
- **このリストは作者が Markdown を読んで承認した(2026-10-18)。** 承認前の要確認のケースは7件(`TC-A` はすべて要確認)、API 改定の提案は0件。TC-A6 は作者の判断で削除した(付録「削除したケース」)。ほかの番号は変えていない。
- このリストは仕様由来ではなく、自分で検証しただけのものである。信用する前に、引用を1〜2件抜き取って確認すること。
- ケースは High → Medium の順に並べてある。
- このシステムの観察できる結果は、プライマリウィンドウの `ime_enabled` と `ime_position`、および `ImeCandidateArea` の3つである。このうち `ImeCandidateArea` の中身は、どの doc comment にも書かれていないので、ケースにしていない(付録「仕様にないもの」)。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-A1 | `a_focused_surface_switches_the_window_ime_on_at_its_cursor_cell` | MN, PE, AC, TL, CM, DW | High | 要確認 |
| TC-A2 | `a_focused_inline_webview_moves_the_window_anchor_to_its_overlay_rect` | PE, AI, WP, OV, FW, TL | High | 要確認 |
| TC-A3 | `a_tab_rename_switches_the_window_ime_on_without_moving_its_anchor` | PR | High | 要確認 |
| TC-A4 | `no_focused_surface_and_no_webview_switches_the_window_ime_off` | PE, RS | High | 要確認 |
| TC-A5 | `a_vi_mode_surface_switches_the_window_ime_off` | PE, VM | High | 要確認 |
| TC-A7 | `an_inline_webview_on_an_inactive_slot_leaves_the_window_anchor_unchanged` | PE, WP, OV | Medium | 要確認 |
| TC-A8 | `a_focused_inline_webview_keeps_the_window_ime_on_over_a_vi_mode_surface` | PE, AI | Medium | 要確認 |

Source タグ — **DW**: `ime.rs:186`(プライマリウィンドウに書く)／**PE**: `ime.rs:189`(IME が有効になる条件)／**AC**: `ime.rs:193`(カーソルのセルを基準にする)／**AI**: `ime.rs:194`(インラインのウェブビューはオーバーレイ矩形の起点)／**PR**: `ime.rs:199`(タブ名変更中は変更欄が位置を決める)／**RS**: `ime.rs:177`(サーフェスなし)／**WP**: `ime.rs:282`(ウェブビューの位置の式と、位置を変えない場合)／**TL**: `ime.rs:309`(ノードの左上)／**MN**: `main.rs:128`(最初のフレームで true に切り替える)／**OV**: `overlay.rs:15`(オーバーレイ矩形)／**FW**: `mount.rs:166`(アクティブなサーフェスの子)／**VM**: `mode.rs:28`(vi モード)／**CM**: `metrics.rs:89`(セルの大きさは物理 px)。引用の全文は付録の台帳にある。

全ケースが `TC-A` なのは、IME の候補ウィンドウの位置と IME の有効・無効が、`docs/references/` のどのマニュアルの範囲にも入らないためである。

テストコードは `src/input/ime.rs` の `mod tests` へ追加する。このシステムの既存のテストも同じモジュールにある。下の「共通のヘルパ」も同じモジュールに置く。既存のヘルパとは名前が重ならないようにしてある。

**コンパイルと実行について:** Rust はまだビルドしていない。現在の木で型はそろっている見込みだが、`ime_policy_system` が呼ぶ `ImePolicyParams::decide` が `todo!()` なので、どのテストも panic で失敗する。貼り付ければ通るものとしてではなく、書き起こしの案として扱うこと。

## 共通のヘルパ

`mod tests` の先頭の import に、次の2行を足す。

```rust
use bevy::math::IVec4;
use orzma_vt::prelude::{GridColumn, GridLine, GridPoint};
```

```rust
/// The `ime_position` the primary window starts with, distinct from every
/// anchor a case expects.
const UNTOUCHED_POSITION: Vec2 = Vec2::new(-7.0, -7.0);

/// An app holding every resource `ime_policy_system` reads, with 8×16 px
/// cells and a primary window at a scale factor of 2 whose IME starts off
/// at `UNTOUCHED_POSITION`.
fn system_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.init_resource::<FocusedWebview>();
    app.init_resource::<ImeCandidateArea>();
    app.insert_resource(TerminalCellMetricsResource {
        metrics: CellMetrics {
            cell_size: Vec2::new(8.0, 16.0),
            baseline: Baseline::new(12.0),
            underline: Underline {
                position: -2.0,
                thickness: Thickness::new(1.0),
            },
            max_overflow: 0.0,
        },
        phys_font_size: 12,
    });
    app.world_mut().spawn((
        Window {
            focused: true,
            resolution: WindowResolution::new(1600, 1200).with_scale_factor_override(2.0),
            ime_enabled: false,
            ime_position: UNTOUCHED_POSITION,
            ..default()
        },
        PrimaryWindow,
    ));
    app
}

fn switch_window_ime_on(app: &mut App) {
    let mut windows = app
        .world_mut()
        .query_filtered::<&mut Window, With<PrimaryWindow>>();
    windows
        .single_mut(app.world_mut())
        .expect("one primary window")
        .ime_enabled = true;
}

/// Spawns a keyboard-focused terminal whose node's top-left corner sits at
/// (100, 100) physical px, with the cursor at column 5, row 10 of a 80×24
/// viewport.
fn spawn_cursor_surface(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            OrzmaTerminal,
            KeyboardFocused,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(500.0, 400.0),
            TerminalView {
                cols: 80,
                rows: 24,
                cursor: Some(Cursor {
                    point: GridPoint {
                        line: GridLine(10),
                        column: GridColumn(5),
                    },
                    ..Cursor::default()
                }),
                ..default()
            },
            TerminalOverlays::default(),
        ))
        .id()
}

/// Gives `surface`'s overlay slot 0 the cell rect `rect`
/// (`row, col, rows, cols`), spawns a webview on that slot as its child,
/// and focuses the webview.
fn focus_inline_child(app: &mut App, surface: Entity, rect: IVec4) {
    app.world_mut()
        .get_mut::<TerminalOverlays>(surface)
        .expect("the surface carries overlays")
        .rects[0] = rect;
    let webview = app
        .world_mut()
        .spawn((
            ChildOf(surface),
            Webview::new("webview".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
        ))
        .id();
    app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
}

/// Runs the policy once and returns the primary window's IME switch and
/// anchor.
fn run_policy(app: &mut App) -> (bool, Vec2) {
    app.world_mut()
        .run_system_once(ime_policy_system)
        .expect("the policy runs");
    let mut windows = app
        .world_mut()
        .query_filtered::<&Window, With<PrimaryWindow>>();
    let window = windows.single(app.world()).expect("one primary window");
    (window.ime_enabled, window.ime_position)
}
```

## TC-A1 — フォーカスのあるサーフェスでは IME をオンにし、カーソルのセルを基準位置にする

| | |
| - | - |
| Setup | `system_app()`(IME はオフ、拡大率 2、`ime_position` は `UNTOUCHED_POSITION`); `spawn_cursor_surface`(ノードの左上は (100, 100) 物理 px、カーソルは列5・行10) |
| Act | `ime_policy_system` を1回実行する |
| Expect | `ime_enabled` が `false` から `true` になる **[MN][PE]** ／ `ime_position == Vec2::new(70.0, 130.0)`。計算は ((100 + 5×8) / 2, (100 + 10×16) / 2) **[AC][TL][CM][DW]** |

MN は「`ime_enabled` はわざと `false` で始まり、`ime_policy_system` が最初のフォーカスのあるフレームで `true` に切り替える」と述べ、PE は vi モードでないサーフェスがあれば有効だと述べている。2行目は、`decide` の承認済みの TC-A7 と同じく、AC を「カーソルのセルの左上(論理 px)」と読んだものである。旧実装の「1行下にずらす」実装と、拡大率で割り忘れる実装を捕まえる。

```rust
/// Asserts that a keyboard-focused terminal switches the window's IME on
/// and anchors the candidate window at its cursor cell in logical pixels.
///
/// Case: orzma has just opened with the IME off, and the first frame with
/// a focused shell pane runs on a display with a scale factor of 2.
#[test]
fn a_focused_surface_switches_the_window_ime_on_at_its_cursor_cell() {
    let mut app = system_app();
    spawn_cursor_surface(&mut app);

    let (enabled, position) = run_policy(&mut app);

    assert!(enabled, "the first focused-surface tick switches the IME on");
    assert!(
        position.abs_diff_eq(Vec2::new(70.0, 130.0), 1e-3),
        "the anchor is the cursor cell's top-left in logical px, got {position:?}",
    );
}
```

## TC-A2 — インラインのウェブビューにフォーカスがあれば、オーバーレイ矩形の起点を基準位置にする

| | |
| - | - |
| Setup | `system_app()`; `spawn_cursor_surface`; `focus_inline_child` で slot 0 を (row 2, col 3, rows 4, cols 10) にする |
| Act | `ime_policy_system` を1回実行する |
| Expect | `ime_enabled` が `true` になる **[PE]** ／ `ime_position == Vec2::new(62.0, 66.0)`。計算は ((100 + 3×8) / 2, (100 + 2×16) / 2) **[AI][WP][OV][FW][TL]** |

AI によれば、フォーカスのあるウェブビューがアクティブなペインのインラインの子なら、基準位置はその子のオーバーレイ矩形の起点になる。式は WP、矩形の並びは OV による。ターミナルのカーソルを基準にしたままにする実装と、行と列を取り違える実装を捕まえる。

```rust
/// Asserts that a focused inline webview moves the window's anchor to its
/// overlay rect origin in logical pixels.
///
/// Case: the user clicks into an inline webview the active pane mounted at
/// row 2, column 3, on a display with a scale factor of 2.
#[test]
fn a_focused_inline_webview_moves_the_window_anchor_to_its_overlay_rect() {
    let mut app = system_app();
    let surface = spawn_cursor_surface(&mut app);
    focus_inline_child(&mut app, surface, IVec4::new(2, 3, 4, 10));

    let (enabled, position) = run_policy(&mut app);

    assert!(enabled, "a focused webview keeps the IME on");
    assert!(
        position.abs_diff_eq(Vec2::new(62.0, 66.0), 1e-3),
        "the anchor is the overlay rect origin in logical px, got {position:?}",
    );
}
```

## TC-A3 — タブ名変更中は IME をオンにし、基準位置は動かさない

| | |
| - | - |
| Setup | `system_app()`; `TabRename::active_for_test(TabId(1))` を挿入; `spawn_cursor_surface`(vi モードなし) |
| Act | `ime_policy_system` を1回実行する |
| Expect | `ime_enabled` が `true` になる **[PR]** ／ `ime_position` は `UNTOUCHED_POSITION` のまま **[PR]** |

PR は「タブ名変更中は IME を有効に保ち、変更欄が候補ウィンドウの位置を決める」と述べている。このシステムが基準位置を書かないことは、2文目から読める。フォーカスのあるターミナルも置いてあるのは、ターミナルのカーソルで基準位置を上書きしてしまう実装を捕まえるためである。サーフェスを vi モードにしないのは、付録の「doc comment 同士の矛盾」を避けるためである。

```rust
/// Asserts that a tab rename switches the window's IME on and leaves the
/// anchor to the rename field.
///
/// Case: the user double-clicks a tab to rename it while the shell pane
/// below still holds keyboard focus.
#[test]
fn a_tab_rename_switches_the_window_ime_on_without_moving_its_anchor() {
    let mut app = system_app();
    app.insert_resource(TabRename::active_for_test(TabId(1)));
    spawn_cursor_surface(&mut app);

    let (enabled, position) = run_policy(&mut app);

    assert!(enabled, "a tab rename keeps the IME on");
    assert_eq!(position, UNTOUCHED_POSITION);
}
```

## TC-A4 — フォーカスのあるサーフェスもウェブビューもなければ IME をオフにする

| | |
| - | - |
| Setup | `system_app()`; `switch_window_ime_on` で IME をオンにしておく; サーフェス、ウェブビュー、タブ名変更はどれもなし |
| Act | `ime_policy_system` を1回実行する |
| Expect | `ime_enabled` が `false` になる **[PE][RS]** |

PE の「有効」の条件をどちらも満たさない状態である。IME をオンにしてから始めるのは、何も書かない実装(オフのまま残るだけの実装)と区別するためである。

```rust
/// Asserts that the window's IME switches off when neither a terminal nor
/// a webview holds keyboard focus.
///
/// Case: the last pane with focus goes away while the IME was on.
#[test]
fn no_focused_surface_and_no_webview_switches_the_window_ime_off() {
    let mut app = system_app();
    switch_window_ime_on(&mut app);

    let (enabled, _) = run_policy(&mut app);

    assert!(!enabled);
}
```

## TC-A5 — フォーカスのあるサーフェスが vi モードなら IME をオフにする

| | |
| - | - |
| Setup | `system_app()`; `switch_window_ime_on`; `spawn_cursor_surface` に `ViModeState` を付ける |
| Act | `ime_policy_system` を1回実行する |
| Expect | `ime_enabled` が `false` になる **[PE][VM]** |

```rust
/// Asserts that a keyboard-focused terminal in vi mode switches the
/// window's IME off.
///
/// Case: the user enters vi mode in the focused pane while the IME was on.
#[test]
fn a_vi_mode_surface_switches_the_window_ime_off() {
    let mut app = system_app();
    switch_window_ime_on(&mut app);
    let surface = spawn_cursor_surface(&mut app);
    app.world_mut().entity_mut(surface).insert(ViModeState);

    let (enabled, _) = run_policy(&mut app);

    assert!(!enabled);
}
```

## TC-A7 — インラインのウェブビューのオーバーレイが無効な slot なら、IME はオンにして基準位置は動かさない

| | |
| - | - |
| Setup | `system_app()`; `spawn_cursor_surface`; `focus_inline_child` で slot 0 を (row 2, col 3, **rows 0**, cols 10) にする |
| Act | `ime_policy_system` を1回実行する |
| Expect | `ime_enabled` が `true` になる **[PE]** ／ `ime_position` は `UNTOUCHED_POSITION` のまま **[WP][OV]** |

OV によれば `rows == 0` は無効な slot の目印で、WP によれば、そのとき位置は求められず、呼び出し側(このシステム)は `ime_position` を変えない。ターミナルのカーソルにフォールバックして基準位置を書く実装を捕まえる。

```rust
/// Asserts that an inline webview on an inactive overlay slot keeps the
/// window's IME on but leaves its anchor where it was.
///
/// Case: the user focuses an inline webview in the frame its pane
/// released the overlay slot, before the focus moves away.
#[test]
fn an_inline_webview_on_an_inactive_slot_leaves_the_window_anchor_unchanged() {
    let mut app = system_app();
    let surface = spawn_cursor_surface(&mut app);
    focus_inline_child(&mut app, surface, IVec4::new(2, 3, 0, 10));

    let (enabled, position) = run_policy(&mut app);

    assert!(enabled, "a focused webview keeps the IME on");
    assert_eq!(position, UNTOUCHED_POSITION);
}
```

## TC-A8 — インラインのウェブビューにフォーカスがあれば、サーフェスが vi モードでも IME はオンのまま

| | |
| - | - |
| Setup | `system_app()`; `spawn_cursor_surface` に `ViModeState` を付ける; `focus_inline_child` で slot 0 を (row 2, col 3, rows 4, cols 10) にする |
| Act | `ime_policy_system` を1回実行する |
| Expect | `ime_enabled` が `true` になる **[PE][AI]** |

PE は「ウェブビューのフォーカス、**または** vi モードでないサーフェス」なので、ウェブビューのフォーカスだけで有効になる。vi モードを先に判定して IME をオフにする実装を捕まえる。

```rust
/// Asserts that a focused inline webview keeps the window's IME on even
/// when its terminal is in vi mode.
///
/// Case: the user enters vi mode in a pane, then clicks into the inline
/// webview that pane mounted.
#[test]
fn a_focused_inline_webview_keeps_the_window_ime_on_over_a_vi_mode_surface() {
    let mut app = system_app();
    let surface = spawn_cursor_surface(&mut app);
    app.world_mut().entity_mut(surface).insert(ViModeState);
    focus_inline_child(&mut app, surface, IVec4::new(2, 3, 4, 10));

    let (enabled, _) = run_policy(&mut app);

    assert!(enabled);
}
```

## 付録

### 契約表

マニュアル由来のエントリはない(探索した語と結果は次節)。ケースの根拠はすべて、次の種別2の台帳による。

| タグ | 場所 | 引用 | 根拠にするもの |
| - | - | - | - |
| DW | `src/input/ime.rs:186-187`, `ime_policy_system` | "Derives whether IME should be on this tick and writes `PrimaryWindow.ime_enabled` and `.ime_position`." | 結果はプライマリウィンドウの2つのフィールドに書かれる |
| PE | `src/input/ime.rs:189-190`, `ime_policy_system` | "`ime_enabled` is `true` iff a CEF webview owns focus, or a surface exists that does NOT have `ViModeState`." | ウェブビューのフォーカスか、vi モードでないサーフェスがあるときだけ有効 |
| AC | `src/input/ime.rs:193-194`, `ime_policy_system` | "`ime_position` anchors the OS candidate window at the focused surface's cursor cell." | ターミナルでは、カーソルのセルを基準にする |
| AI | `src/input/ime.rs:194-196`, `ime_policy_system` | "When the focused webview is an INLINE child of the active pane, the anchor instead comes from that child's overlay rect origin (`webview_ime_position`)" | インラインのウェブビューは、オーバーレイ矩形の起点を基準にする |
| PR | `src/input/ime.rs:199-200`, `ime_policy_system` | "While a tab rename is active IME stays enabled and the rename field positions the candidate window." | タブ名変更中は有効で、位置は変更欄が決める |
| RS | `src/input/ime.rs:177-178`, `resolve_focused_surface` | "Returns `None` when zero or more than one entity is focused; both degrade to "no surface"." | フォーカスが0個か2個以上なら、サーフェスなし |
| WP | `src/input/ime.rs:282-287`, `webview_ime_position` | "The logical-pixel anchor for the OS candidate window when a webview owns focus: the owning terminal node's top-left (physical px) plus the child's active overlay rect origin (`rect.y × cell_w`, `rect.x × cell_h`), divided by the window scale factor. `None` when the focus chain is gone (child/terminal despawned, no terminal node, or a sentinel rect) — the caller then leaves `ime_position` unchanged rather than mis-anchoring." | インラインのウェブビューの位置の式と、無効な矩形のときに位置を変えないこと |
| TL | `src/input/ime.rs:309, 314-315`, `node_top_left_phys` | "The top-left corner of `node`, in physical pixels of window space." / "a caller that needs logical pixels divides it." | ノードの左上は物理 px で、論理 px には拡大率で割る |
| MN | `src/main.rs:125-129`, `primary_window` | "`ime_enabled` starts `false` deliberately: … `ime_policy_system` flips it to `true` on the first focused-surface tick, producing the arming transition." | 最初のフォーカスのあるフレームで `false` から `true` に切り替える |
| OV | `crates/bevy_orzma_tty_renderer/src/material/overlay.rs:15-17`, `TerminalOverlays` | "`rects[i]` is `(row, col, rows, cols)` in CELL coordinates; `row` may be negative when the rect starts above the viewport. `rows == 0` is the inactive-slot sentinel" | 矩形の並びと、無効な slot の目印 |
| FW | `crates/bevy_orzma_webview/src/webview/mount.rs:166-168`, `focused_webview_of` | "Returns the webview entity that currently holds keyboard focus on `active_surface`: `Some(e)` iff `FocusedWebview` points at `e`, `e` carries `Webview`, and its `ChildOf` parent is the active surface." | 「アクティブなサーフェスの子」の定義 |
| VM | `src/action/vi/mode.rs:28`, `ViModeState` | "Marker: presence on a Surface entity means "vi mode is active"." | vi モードの定義 |
| CM | `crates/bevy_orzma_tty_renderer/src/font/metrics.rs:89`, `TerminalCellMetricsResource::metrics` | "Current cell pitch and typographic measurements in physical pixels." | セルの大きさは物理 px |

### 探索した語

| 段 | 出どころ | 語 | 結果 |
| - | - | - | - |
| 1 | `ime_policy_system` の `///` | IME | 全マニュアルで0件 |
| 1 | 同上 | candidate window | 全マニュアルで0件 |
| 1 | 同上 | webview | 全マニュアルで0件 |
| 1 | 同上 | vi mode | VT510 4件(すべて「TVI mode」)。除外 |
| 1 | 同上 | rename | Xlib 16件(`XStoreNamedColor` など)。除外 |
| 1 | 同上 | cursor cell | 全マニュアルで0件 |
| 2 | 囲む型の doc | (なし) | 自由関数なので該当なし |
| 3 | `src/input/ime.rs` の `//!` | IME / composition / keyboard focus | 前回の `decide` の実行と同じ結果。IME は0件、composition は振る舞いを述べない記述だけ、keyboard focus は X のクライアントと入力メソッドの層 |
| 4 | `Window` → candidate box | candidate box | 全マニュアルで0件 |
| 4 | `ImeCandidateArea` / `ImePolicyParams` → input method / preedit | input method / preedit | Xlib 314件 / 226件(第13章の XIM)。X のクライアントと入力メソッドの間の規定で、このシステムを規定しない |

### 仕様にないもの

根拠となる doc comment がないので、ケースにしていない。

- **`ImeCandidateArea` の中身。** どの状態で `Some` にし、どの状態で `None` にするか、`Some` のときの矩形が何を表すか。`ImeCandidateArea` にも `ImeTarget` にも doc comment がない(バリアントの右の `//` のコメントは doc comment ではないので根拠にできない)。今回の変更の中心なので、doc comment で契約を書けば、ここが最も多くのケースになる。
- 値が変わらないときに、`ime_enabled`、`ime_position`、`ImeCandidateArea` を変更として記録しないこと。
- プライマリウィンドウがないときの振る舞い。
- フォーカスのあるウェブビューが、キーボードフォーカスを持たない別ペインの子であるとき、基準位置をどうするか。
- カーソルが表示範囲の外にあるとき、またはカーソルがないときの基準位置(要件 R6 に関係)。
- サーフェスに UI ノードがないとき、`scale_factor` が 0 以下のとき。

### 仕様の矛盾

- タブ名変更中に、vi モードのサーフェスがあり、ウェブビューがないとき、PE(`ime.rs:189`)は「無効」、PR(`ime.rs:199`)は「有効」になる。どちらを優先するかは決められないので、ケースを作っていない。

### 別の層を規定するマニュアルの記述

- `xlib.pdf` p.320, L18603-18606(verify 済み): "The XNSpotLocation argument specifies to the input method the coordinates of the spot to be used by an input method executing with XNInputStyle set to XIMPreeditPosition."
- `xlib.pdf` p.320, L18609-18610(verify 済み): "The y coordinate is the position of the baseline used by the current text line in the focus window."

X11 では winit が `ime_position` をそのまま spot として渡すので、TC-A1 の基準位置(カーソルのセルの左上)は、Xlib の「ベースライン」とずれる。X11 の扱いを決めるときの材料になる。

### 出典なしの改善提案

- `ImeCandidateArea` と `ImeTarget` に doc comment で契約を書けば、「仕様にないもの」の先頭の項目をケースにできる。
- `ime_policy_system` の doc comment は、`webview_anchors` で読むタブ単位のウェブビュー(本番コードには存在しない)を前提にした書き方が残っている。

### 削除したケース

- **TC-A6**(`two_focused_surfaces_switch_the_window_ime_off`):`KeyboardFocused` を持つサーフェスが2つある状態を前提にしていたが、作者の指摘を受けてコードを確かめたところ、その状態は起きない。本番コードで `KeyboardFocused` を付けるのは `on_active_pane_changed` だけで(`src/input/focus.rs:204`)、付ける前に同じ observer の中でほかのすべてから外している(`focus.rs:191-195`)。`KeyboardFocused` の doc comment も「exactly one focused」の不変条件を述べている(`focus.rs:33`)。作者の判断で削除した。

### 転記中の気づき

ケースを書き起こすためにヘルパを探す中で、`src/input/ime.rs` の既存のテストについて次のことに気づいた。リストには加えていない。

- `ime_enabled_and_anchored_for_focused_orzma_terminal` は、基準位置が1行下の `(0, 16)` であることを確かめている。TC-A1(カーソルのセルの左上)と食い違うので、実装するときは直すか削除する必要がある。
- `ime_stays_enabled_for_focused_webview` は、`spawn_empty()` で作った、`Webview` も親も持たないエンティティに `FocusedWebview` を向けている。`decide` の TC-A2 を削除したときに確かめたとおり、本番では起きない状態である。
- `run_system_once(ime_policy_system)` を呼ぶ既存のテスト5件は、`ImeCandidateArea` を初期化していないので、今は Resource が見つからずに失敗する。
- `ime_position_anchors_at_inline_rect_origin_for_focused_inline` と `the_ime_policy_yields_to_the_rename_field` は、TC-A2 と TC-A3 と同じ状況を扱っている。新しいケースを入れるなら、重複をどうするか決める必要がある。
