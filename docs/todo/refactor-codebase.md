`orzma_tty`より上流のクレート全てのコード設計を確認する

- [x] orzmux
- [ ] bevy_orzma_tty_renderer
- [ ] bevy_orzma_webview_host
- [ ] bevy_orzma_webview
- [ ] bevy_orzmux
- [ ] orzma

- `GridSize` のフィールドを private にし、型で軸 0 と幅 1 列を防ぐ。今は `DeviceState::resize` が軸 0 のサイズを無視し、幅 1 列を `MIN_COLUMNS` に上げている。
