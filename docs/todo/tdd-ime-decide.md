# Test cases: ImePolicyParams::decide

状態: 承認済み

`src/input/ime.rs` の `ImePolicyParams::decide(&self, scale_factor: f32) -> ImeTarget` が負うテストケース。`docs/references/` のマニュアルに `decide` を規定する記述はなかったため、すべてのケースはリポジトリ内の doc comment(種別2)を根拠にした `TC-A` である。仕様(マニュアル)由来のケースは0件。

- 引用の検証: **14/14**(リポジトリの doc comment 12件を `grep -F` で、付録で参照する `xlib.pdf` の2件を `verify` で確認)
- **このリストは作者が Markdown を読んで承認した(2026-10-18)。** 承認前の要確認のケースは9件(`TC-A` はすべて要確認)、API 改定の提案は0件。TC-A2 は作者の判断で削除した(付録「削除したケース」)。ほかの番号は変えていない。
- このリストは仕様由来ではなく、自分で検証しただけのものである。信用する前に、引用を1〜2件抜き取って確認すること。
- ケースは High → Medium の順に並べてある。High だけ読めば、doc comment が明言している振る舞いはすべてそろう。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-A1 | `a_tab_rename_in_progress_yields_the_rename_target` | PR, RA | High | 要確認 |
| TC-A3 | `an_inline_webview_anchors_at_its_overlay_rect_origin` | AI, WP, OV, FW, TL | High | 要確認 |
| TC-A4 | `no_focused_surface_and_no_webview_disables_the_ime` | PE, RS | High | 要確認 |
| TC-A5 | `two_keyboard_focused_surfaces_disable_the_ime` | RS, PE | High | 要確認 |
| TC-A6 | `a_focused_surface_in_vi_mode_disables_the_ime` | PE, VM | High | 要確認 |
| TC-A7 | `a_focused_surface_anchors_the_candidate_area_at_its_cursor_cell` | PE, AC, TL, CM | High | 要確認 |
| TC-A8 | `a_tab_rename_asked_to_end_this_frame_still_yields_the_rename_target` | RA, PR | Medium | 要確認 |
| TC-A9 | `an_inline_webview_on_an_inactive_overlay_slot_leaves_the_anchor_unset` | WP, OV, PE | Medium | 要確認 |
| TC-A10 | `a_focused_inline_webview_keeps_the_ime_enabled_over_a_vi_mode_surface` | PE, AI | Medium | 要確認 |

Source タグ — **PE**: `ime.rs:221`(IME が有効になる条件)／**AC**: `ime.rs:225`(カーソルのセルを基準にする)／**AI**: `ime.rs:226`(インラインのウェブビューはオーバーレイ矩形の起点)／**PR**: `ime.rs:231`(タブ名変更中)／**RS**: `ime.rs:209`(サーフェスなし)／**WP**: `ime.rs:314`(ウェブビューの位置の式)／**OV**: `overlay.rs:15`(オーバーレイ矩形の並びと無効 slot)／**FW**: `mount.rs:166`(アクティブなサーフェスの子)／**VM**: `mode.rs:28`(vi モード)／**RA**: `rename.rs:29`(終了を求められた回も進行中)／**TL**: `ime.rs:341`(ノードの左上)／**CM**: `metrics.rs:89`(セルの大きさは物理 px)。引用の全文は付録の台帳にある。

全ケースが `TC-A` なのは、IME の候補ウィンドウの位置が `docs/references/` のどのマニュアルの範囲にも入らないためである。マニュアルで唯一関係しそうな Xlib の `XNSpotLocation` は、`decide` ではなく X11 のクライアントと入力メソッドの間の値を規定している(付録「別の層を規定するマニュアルの記述」)。

テストコードは `src/input/ime.rs` の `mod tests` へ追加する。既存のテストと同じく、`MinimalPlugins` と `StatesPlugin` を入れた `App` に `TerminalCellMetricsResource` を挿入して使う。下の「共通のヘルパ」も同じモジュールに置く。

**コンパイルと実行について:** Rust はまだビルドしていない。現在の木で型はそろっている見込みだが、`decide` が `todo!()` なので、どのテストも panic で失敗する。貼り付ければ通るものとしてではなく、書き起こしの案として扱うこと。`ImeTarget` が `Debug` / `PartialEq` を derive していないので、比較は `matches!` と `let … else` で書いてある。

## 共通のヘルパ

```rust
use bevy::math::IVec4;
use orzma_vt::prelude::{GridColumn, GridLine, GridPoint};

fn policy_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.init_resource::<FocusedWebview>();
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
    app
}

/// Spawns a keyboard-focused terminal whose node's top-left corner sits at
/// (100, 100) physical px, with the cursor at column 5, row 10 of a 80×24
/// viewport.
fn spawn_focused_surface(app: &mut App) -> Entity {
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
fn focus_inline_webview(app: &mut App, surface: Entity, rect: IVec4) -> Entity {
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
    webview
}

fn decide_in(app: &mut App, scale_factor: f32) -> ImeTarget {
    app.world_mut()
        .run_system_once(move |params: ImePolicyParams| params.decide(scale_factor))
        .expect("ImePolicyParams resolves against the test world")
}
```

## TC-A1 — タブ名変更中は、ターミナルにフォーカスがあっても Rename を返す

| | |
| - | - |
| Setup | `policy_app()`; `TabRename::active_for_test(TabId(1))` を挿入; `spawn_focused_surface`(vi モードなし) |
| Act | `params.decide(1.0)` |
| Expect | `ImeTarget::Rename` を返す **[PR][RA]** |

PR は「タブ名変更中は、変更欄が候補ウィンドウの位置を決める」と述べている。vi モードでないターミナルも置いてあるのは、ターミナルを先に判定して `Terminal` を返してしまう実装を捕まえるためである。サーフェスを vi モードにしないのは、PE と PR がどちらも「有効」と言う状態にして、付録の「doc comment 同士の矛盾」を避けるためである。`TabRename::active_for_test` は `src/ui/tab_bar/rename.rs:66` にある。

```rust
/// Asserts that a tab rename in progress yields the rename target even
/// while a terminal outside vi mode holds keyboard focus.
///
/// Case: the user double-clicks a tab to rename it while the shell pane
/// below still holds keyboard focus.
#[test]
fn a_tab_rename_in_progress_yields_the_rename_target() {
    let mut app = policy_app();
    app.insert_resource(TabRename::active_for_test(TabId(1)));
    spawn_focused_surface(&mut app);

    let target = decide_in(&mut app, 1.0);

    assert!(matches!(target, ImeTarget::Rename));
}
```

## TC-A3 — インラインのウェブビューは、オーバーレイ矩形の起点を論理ピクセルで返す

| | |
| - | - |
| Setup | `policy_app()`; `spawn_focused_surface`(ノードの左上は (100, 100) 物理 px); `focus_inline_webview` で slot 0 を (row 2, col 3, rows 4, cols 10) にする; セルは 8×16 物理 px |
| Act | `params.decide(2.0)` |
| Expect | `ImeTarget::Webview(Some(pos))` を返し、`pos == Vec2::new(62.0, 66.0)`。計算は ((100 + 3×8) / 2, (100 + 2×16) / 2) **[AI][WP][OV][FW][TL]** |

WP の式は「ノードの左上(物理 px) + (`rect.y × cell_w`, `rect.x × cell_h`)を拡大率で割る」で、OV によれば矩形の並びは (row, col, rows, cols) である。行と列を別々の値にしたのは、行と列を取り違える実装を捕まえるためである。ノードの左上を (100, 100) にしたのはノードの位置を足し忘れる実装を、拡大率 2.0 にしたのは拡大率で割り忘れる実装を捕まえるためである。

```rust
/// Asserts that an inline webview anchors at its overlay rect origin,
/// converted to logical pixels.
///
/// Case: the user clicks into an inline webview the active pane mounted at
/// row 2, column 3, on a display with a scale factor of 2.
#[test]
fn an_inline_webview_anchors_at_its_overlay_rect_origin() {
    let mut app = policy_app();
    let surface = spawn_focused_surface(&mut app);
    focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 4, 10));

    let target = decide_in(&mut app, 2.0);

    let ImeTarget::Webview(Some(pos)) = target else {
        panic!("an inline webview with a live overlay rect yields its anchor");
    };
    assert!(
        pos.abs_diff_eq(Vec2::new(62.0, 66.0), 1e-3),
        "the anchor is (node top-left + (col × cell_w, row × cell_h)) / scale, got {pos:?}",
    );
}
```

## TC-A4 — フォーカスのあるサーフェスもウェブビューもなければ Disabled

| | |
| - | - |
| Setup | `policy_app()` のみ(`KeyboardFocused` なし、`FocusedWebview(None)`、`TabRename` なし) |
| Act | `params.decide(1.0)` |
| Expect | `ImeTarget::Disabled` を返す **[PE][RS]** |

PE の「有効」の条件(ウェブビューのフォーカス、または vi モードでないサーフェス)のどちらも満たさない、もっとも基本的な状態である。

```rust
/// Asserts that the IME is disabled when neither a terminal nor a webview
/// holds keyboard focus.
///
/// Case: the window has just opened and no pane has taken focus yet.
#[test]
fn no_focused_surface_and_no_webview_disables_the_ime() {
    let mut app = policy_app();

    let target = decide_in(&mut app, 1.0);

    assert!(matches!(target, ImeTarget::Disabled));
}
```

## TC-A5 — フォーカスのあるサーフェスが2つあれば Disabled

| | |
| - | - |
| Setup | `policy_app()`; `spawn_focused_surface` を2回(どちらも vi モードなし); ウェブビューもタブ名変更もなし |
| Act | `params.decide(1.0)` |
| Expect | `ImeTarget::Disabled` を返す **[RS][PE]** |

RS によれば、フォーカスが2つあるときは「サーフェスなし」として扱う。最初に見つかったサーフェスを選んで `Terminal` を返す実装を捕まえる。

```rust
/// Asserts that two keyboard-focused terminals degrade to no focused
/// surface, which disables the IME.
///
/// Case: a focus handoff between two panes momentarily leaves both marked
/// as keyboard-focused.
#[test]
fn two_keyboard_focused_surfaces_disable_the_ime() {
    let mut app = policy_app();
    spawn_focused_surface(&mut app);
    spawn_focused_surface(&mut app);

    let target = decide_in(&mut app, 1.0);

    assert!(matches!(target, ImeTarget::Disabled));
}
```

## TC-A6 — フォーカスのあるサーフェスが vi モードなら Disabled

| | |
| - | - |
| Setup | `policy_app()`; `spawn_focused_surface` に `ViModeState` を付ける; ウェブビューもタブ名変更もなし |
| Act | `params.decide(1.0)` |
| Expect | `ImeTarget::Disabled` を返す **[PE][VM]** |

PE は「vi モードでないサーフェス」を有効の条件にしている。VM によれば、`ViModeState` が付いていることが vi モードの意味である。

```rust
/// Asserts that a keyboard-focused terminal in vi mode disables the IME.
///
/// Case: the user enters vi mode in the focused pane to select scrollback
/// text with the keyboard.
#[test]
fn a_focused_surface_in_vi_mode_disables_the_ime() {
    let mut app = policy_app();
    let surface = spawn_focused_surface(&mut app);
    app.world_mut().entity_mut(surface).insert(ViModeState);

    let target = decide_in(&mut app, 1.0);

    assert!(matches!(target, ImeTarget::Disabled));
}
```

## TC-A7 — vi モードでないサーフェスは、カーソルのセルを候補ウィンドウ用の矩形として返す

| | |
| - | - |
| Setup | `policy_app()`; `spawn_focused_surface`(ノードの左上は (100, 100) 物理 px、カーソルは列5・行10で表示範囲の中); セルは 8×16 物理 px |
| Act | `params.decide(2.0)` |
| Expect | `ImeTarget::Terminal(Some(rect))` を返す **[PE][AC]** ／ `rect.min == Vec2::new(70.0, 130.0)`。計算は ((100 + 5×8) / 2, (100 + 10×16) / 2) **[AC][TL][CM]** ／ `rect.size() == Vec2::new(4.0, 8.0)`(セル1つ分を論理 px で) **[AC][CM]** |

**この case の2行目と3行目は、AC の「カーソルのセルを基準にする」を「矩形 = カーソルのセル(論理 px)」と読んだものである。** モックでは、矩形の起点の行、幅、単位がまだ決まっていない。幅を変換中の文字列の幅にする場合や、単位を変える場合は、この2行を直すこと。この case は、旧実装の「1行下にずらす」実装と、拡大率で割り忘れる実装を捕まえる。

```rust
/// Asserts that a keyboard-focused terminal outside vi mode yields its
/// cursor cell, in logical pixels, as the candidate area.
///
/// Case: the user starts composing Japanese text at the shell prompt on a
/// display with a scale factor of 2.
#[test]
fn a_focused_surface_anchors_the_candidate_area_at_its_cursor_cell() {
    let mut app = policy_app();
    spawn_focused_surface(&mut app);

    let target = decide_in(&mut app, 2.0);

    let ImeTarget::Terminal(Some(rect)) = target else {
        panic!("a focused terminal outside vi mode yields a candidate area");
    };
    assert!(
        rect.min.abs_diff_eq(Vec2::new(70.0, 130.0), 1e-3),
        "the area starts at the cursor cell's top-left in logical px, got {:?}",
        rect.min,
    );
    assert!(
        rect.size().abs_diff_eq(Vec2::new(4.0, 8.0), 1e-3),
        "the area is one cell in logical px, got {:?}",
        rect.size(),
    );
}
```

## TC-A8 — そのフレームで終了を求められたタブ名変更も、まだ Rename

| | |
| - | - |
| Setup | `policy_app()`; `TabRename::ending_for_test(TabId(1), Entity::PLACEHOLDER, Entity::PLACEHOLDER, true)` を挿入; `spawn_focused_surface`(vi モードなし) |
| Act | `params.decide(1.0)` |
| Expect | `ImeTarget::Rename` を返す **[RA][PR]** |

RA は `is_active` について「終了を求められたフレームも含めて進行中」と明記している。終了を求められた時点で `Terminal` に切り替える実装を捕まえる。`TabRename::ending_for_test` は `src/ui/tab_bar/rename.rs:79` にある。

```rust
/// Asserts that a tab rename asked to end this frame still yields the
/// rename target.
///
/// Case: the user presses Enter to commit a new tab name, and the policy
/// runs in the same frame before the rename finishes.
#[test]
fn a_tab_rename_asked_to_end_this_frame_still_yields_the_rename_target() {
    let mut app = policy_app();
    app.insert_resource(TabRename::ending_for_test(
        TabId(1),
        Entity::PLACEHOLDER,
        Entity::PLACEHOLDER,
        true,
    ));
    spawn_focused_surface(&mut app);

    let target = decide_in(&mut app, 1.0);

    assert!(matches!(target, ImeTarget::Rename));
}
```

## TC-A9 — インラインのウェブビューのオーバーレイが無効な slot なら Webview(None)

| | |
| - | - |
| Setup | TC-A3 と同じ。ただし slot 0 を (row 2, col 3, **rows 0**, cols 10) にする |
| Act | `params.decide(2.0)` |
| Expect | `ImeTarget::Webview(None)` を返す **[WP][OV][PE]** |

OV によれば `rows == 0` は無効な slot の目印で、WP によれば、無効な矩形では位置を求めず、呼び出し側は `ime_position` を変えない。一方、ウェブビューにフォーカスがあるので、PE により IME は有効のままである。ターミナルのカーソルにフォールバックする実装や、`Disabled` を返す実装を捕まえる。

```rust
/// Asserts that an inline webview whose overlay slot is the inactive
/// sentinel keeps the IME enabled but yields no anchor.
///
/// Case: the user focuses an inline webview in the frame its pane released
/// the overlay slot, before the focus moves away.
#[test]
fn an_inline_webview_on_an_inactive_overlay_slot_leaves_the_anchor_unset() {
    let mut app = policy_app();
    let surface = spawn_focused_surface(&mut app);
    focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 0, 10));

    let target = decide_in(&mut app, 2.0);

    assert!(matches!(target, ImeTarget::Webview(None)));
}
```

## TC-A10 — インラインのウェブビューにフォーカスがあれば、サーフェスが vi モードでも有効のまま

| | |
| - | - |
| Setup | TC-A3 と同じで、サーフェスに `ViModeState` を付ける |
| Act | `params.decide(2.0)` |
| Expect | `ImeTarget::Webview(Some(_))` を返す **[PE][AI]** |

PE は「ウェブビューのフォーカス、**または** vi モードでないサーフェス」なので、ウェブビューのフォーカスだけで有効になる。vi モードかどうかを先に判定して `Disabled` を返す実装を捕まえる。

```rust
/// Asserts that a focused inline webview keeps the IME enabled even when
/// its terminal is in vi mode.
///
/// Case: the user enters vi mode in a pane, then clicks into the inline
/// webview that pane mounted.
#[test]
fn a_focused_inline_webview_keeps_the_ime_enabled_over_a_vi_mode_surface() {
    let mut app = policy_app();
    let surface = spawn_focused_surface(&mut app);
    app.world_mut().entity_mut(surface).insert(ViModeState);
    focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 4, 10));

    let target = decide_in(&mut app, 2.0);

    assert!(matches!(target, ImeTarget::Webview(Some(_))));
}
```

## 付録

### 契約表

マニュアル由来のエントリはない(探索した語と結果は次節)。ケースの根拠はすべて、次の種別2の台帳による。

| タグ | 場所 | 引用 | 根拠にするもの |
| - | - | - | - |
| PE | `src/input/ime.rs:221-222`, `ime_policy_system` | "`ime_enabled` is `true` iff a CEF webview owns focus, or a surface exists that does NOT have `ViModeState`." | ウェブビューのフォーカスか、vi モードでないサーフェスがあるときだけ有効 |
| AC | `src/input/ime.rs:225-226`, `ime_policy_system` | "`ime_position` anchors the OS candidate window at the focused surface's cursor cell." | ターミナルでは、カーソルのセルを基準にする |
| AI | `src/input/ime.rs:226-228`, `ime_policy_system` | "When the focused webview is an INLINE child of the active pane, the anchor instead comes from that child's overlay rect origin (`webview_ime_position`)" | インラインのウェブビューは、オーバーレイ矩形の起点を基準にする |
| PR | `src/input/ime.rs:231-232`, `ime_policy_system` | "While a tab rename is active IME stays enabled and the rename field positions the candidate window." | タブ名変更中は `Rename` |
| RS | `src/input/ime.rs:209-210`, `resolve_focused_surface` | "Returns `None` when zero or more than one entity is focused; both degrade to "no surface"." | フォーカスが0個か2個以上なら、サーフェスなし |
| WP | `src/input/ime.rs:314-319`, `webview_ime_position` | "The logical-pixel anchor for the OS candidate window when a webview owns focus: the owning terminal node's top-left (physical px) plus the child's active overlay rect origin (`rect.y × cell_w`, `rect.x × cell_h`), divided by the window scale factor. `None` when the focus chain is gone (child/terminal despawned, no terminal node, or a sentinel rect) — the caller then leaves `ime_position` unchanged rather than mis-anchoring." | インラインのウェブビューの位置の式と、無効な矩形のときの `None` |
| OV | `crates/bevy_orzma_tty_renderer/src/material/overlay.rs:15-17`, `TerminalOverlays` | "`rects[i]` is `(row, col, rows, cols)` in CELL coordinates; `row` may be negative when the rect starts above the viewport. `rows == 0` is the inactive-slot sentinel" | 矩形の並びと、無効な slot の目印 |
| FW | `crates/bevy_orzma_webview/src/webview/mount.rs:166-168`, `focused_webview_of` | "Returns the webview entity that currently holds keyboard focus on `active_surface`: `Some(e)` iff `FocusedWebview` points at `e`, `e` carries `Webview`, and its `ChildOf` parent is the active surface." | 「アクティブなサーフェスの子」の定義 |
| VM | `src/action/vi/mode.rs:28`, `ViModeState` | "Marker: presence on a Surface entity means "vi mode is active"." | vi モードの定義 |
| RA | `src/ui/tab_bar/rename.rs:29-30`, `TabRename::is_active` | "Whether a rename is in progress, including one asked to end this frame." | 終了を求められたフレームも進行中 |
| TL | `src/input/ime.rs:341, 346-347`, `node_top_left_phys` | "The top-left corner of `node`, in physical pixels of window space." / "a caller that needs logical pixels divides it." | ノードの左上は物理 px で、論理 px には拡大率で割る |
| CM | `crates/bevy_orzma_tty_renderer/src/font/metrics.rs:89`, `TerminalCellMetricsResource::metrics` | "Current cell pitch and typographic measurements in physical pixels." | セルの大きさは物理 px |

### 探索した語

| 段 | 出どころ | 語 | 結果 |
| - | - | - | - |
| 1 | `decide` の `///` | (なし) | `NODOC` |
| 2 | `ImePolicyParams` / その `impl` / `ImeTarget` の doc | (なし) | doc comment なし |
| 3 | `src/input/ime.rs` の `//!` | IME | 全マニュアルで0件 |
| 3 | 同上 | composition | ECMA-48 4件(Annex C の組版用語)、VT220 1件(文字コードの構成)、Xlib 3件(中国語入力の説明文)。いずれも振る舞いを述べていないので除外 |
| 3 | 同上 | keyboard focus | Xlib 3件(`XSetICFocus` など、X のクライアントが入力メソッドにフォーカスを伝える層)。`decide` を規定しない |
| 4 | `Ime` → input method | input method | Xlib 314件(第13章の XIM)。X のクライアントと入力メソッドの間の規定で、`decide` を規定しない |
| 4 | `ImeTarget` → preedit / pre-edit | preedit / pre-edit | Xlib 226件 / 0件。同上 |
| 4 | 候補ウィンドウ → candidate | candidate | ECMA-48 1件(ESC Fs の登録手続き)。除外 |
| 4 | 同上 | XNSpotLocation / spot location / over-the-spot | Xlib 4件 / 3件 / 1件。付録「別の層を規定するマニュアルの記述」へ |
| 4 | `scale_factor` → scale factor | scale factor | Xlib 1件。IME と無関係 |

### 仕様にないもの

根拠となる doc comment がないので、ケースにしていない。

- フォーカスのあるウェブビューが、キーボードフォーカスを持たない別ペインの子であるとき、`Webview` と `Terminal` のどちらを返すか。別ペインのウェブビューにフォーカスを移すと、`request_webview_focus` はペインの選択を要求するだけで(`crates/bevy_orzma_webview/src/webview/focus.rs:89-91`)、`KeyboardFocused` が移るのは `OrzmuxActivePaneChanged` を受けた後になる(`src/input/focus.rs:185-206`)。そのため、この状態は一時的に起きうる。
- サーフェスに UI ノードがないときの結果(`Terminal(None)` かどうか)。
- カーソルが表示範囲の外にあるとき、またはカーソルがないときの基準位置(要件 R6 に関係)。`TerminalView::cursor_viewport_cell_or_top` の doc にこの振る舞いは書かれているが、`decide` がそれに従うと書いた doc comment はない。
- `scale_factor` が 0 以下のときの扱い。
- 矩形の幅を変換中の文字列に合わせるか、起点の行をどうするか(AC の「カーソルのセル」より先のこと)。

### 仕様の矛盾

- タブ名変更中に、vi モードのサーフェスがあり、ウェブビューがないとき、PE(`ime.rs:221`)は「無効」、PR(`ime.rs:231`)は「有効」になる。どちらも同じ doc comment の中の記述で、どちらを優先するかは決められないので、ケースを作っていない。

### 別の層を規定するマニュアルの記述

- `xlib.pdf` p.320, L18603-18606(verify 済み): "The XNSpotLocation argument specifies to the input method the coordinates of the spot to be used by an input method executing with XNInputStyle set to XIMPreeditPosition."
- `xlib.pdf` p.320, L18609-18610(verify 済み): "The y coordinate is the position of the baseline used by the current text line in the focus window."

これは X11 のクライアントが入力メソッドに渡す spot の定義で、`decide` の返り値を直接規定するものではないので、ケースにしていない。ただし X11 では、winit が `ime_position` をそのまま spot として渡す。そのため `ImeTarget::Terminal` の矩形の起点を行の上端にすると、Xlib の「ベースライン」とずれる。X11 の扱いを決めるときの材料になる。

### 出典なしの改善提案

- `ImeTarget` に `#[derive(Debug, PartialEq)]` があれば、`assert_eq!` で比べられ、失敗時に値が表示される。
- `decide` と `ImeTarget` の各バリアントに doc comment で契約を書けば、「仕様にないもの」の多くをケースにできる。
- 本番コードの `Webview` はすべてペインの子として作られ(`mount.rs:342-356`)、UI ノードを持たない(`mount.rs:335-341`)。そのため、ウェブビューの UI ノードから位置を取る旧 `webview_anchors` の分岐が扱っていた「タブ単位のウェブビュー」は存在しない。`ImePolicyParams::webview_anchors` は不要の可能性がある。

### 削除したケース

- **TC-A2**(`a_focused_webview_without_a_focused_surface_keeps_the_ime_enabled`):親を持たない `Webview` にフォーカスがある状態を前提にしていたが、作者の指摘を受けてコードを確かめたところ、その状態は起きない。`Webview` を作るのは `spawn_webview` だけで、必ず `ChildOf(pane)` を付け、ペインが消えていれば何も作らない(`crates/bevy_orzma_webview/src/webview/mount.rs:319-322, 342-356`)。また、ペインとともに `Webview` が消えると、`release_removed_focus` が `FocusedWebview` を外す(`focus.rs:123-129`)。作者の判断で削除した。
