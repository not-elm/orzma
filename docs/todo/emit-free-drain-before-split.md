# split の直前に、イベントを出さずに保留チャンクを解釈する経路

由来: Windows の CWD 継承作業（`docs/superpowers/plans/2026-09-20-windows-split-pane-cwd.md` Task 4）で
一度実装し、撤回した設計。

## 何が欲しかったか

`on_new_pane` が split 対象の `Pane::cwd()` を読む前に、その対象の**未解釈のチャンクを解釈**して
`reported_cwd` を最新にしたい。`wait_ready()` の `Select` はコマンドとペイン出力を同列に待つため、
届いているが解釈されていない `OSC 7` / `OSC 9;9` を取りこぼし得る。
Windows では報告が最優先候補なので、ここが主経路になる。

## なぜ撤回したか

素直に `pump_pane(target)` を呼ぶと、**一度も pump されていないペインの「未払いの bootstrap frame」が
強制的に吐き出される**。

- `crates/orzma_tty/src/lib.rs:508` — `needs_bootstrap() || is_due(now)` で frame を出す
- `crates/orzma_tty/src/coalescer.rs:67` — 新規 coalescer は常に bootstrap を要求する

結果、`PaneOpened` / `Layout` より前に単独の `OrzmuxEvent::Frame` が飛び、既存テスト 12 本が落ちた。
うち `a_split_resizes_the_target_and_bundles_its_frame_with_the_layout` は
**「レイアウト変更に伴う frame は `Layout` イベントに同梱される」という backend の実動作契約**を
固定しているので、これはテストの過剰仕様ではなく製品挙動の破壊だった。

## 正しい形

`OrzmaTty` に、**キュー済みチャンクを解釈してシグナルだけ返し、coalescer の frame 債務には触れない**
メソッドを足す。frame は次の通常 pump（deadline サービスか `Select` の起床）でこれまでどおり
`Layout` に同梱されて出る。`resolve_at` はそれを呼んで `reported_cwd` を更新する。

既存の `pump()` / `flush_now()` の契約は変えない。新しいメソッドとして足すこと。

## 取りこぼしの実害

split コマンドと同一瞬間に届いた報告のみ。報告はプロンプト描画時に出るので人間がショートカットを
押すより十分前であり、`Select` ループは常時 drain しているため窓は 1 ループ分。
計画自身が drain を "bounded best-effort" と書いており、Task 4 の範囲でこの設計変更を買う価値は
無いと判断した。

## 判断

`orzma_tty` の公開 API を増やす変更なので、CWD 継承とは独立に判断する。
