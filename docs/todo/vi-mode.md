ターミナルにVIモードを実装する。

- リサイズのリフロー（`Screen::reflow`）で、vi カーソルを `TrackedPoint` として新しい位置へ移す。
