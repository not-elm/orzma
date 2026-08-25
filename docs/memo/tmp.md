いっそのことFrameTracker(仮名)のような構造体をframe.rsに宣言し、

```rust
pub(crate) struct FrameTracker{
  damage: DamageLedger,
  prev_cursor: Option<Cursor>,
  ...
}

impl FrameTracker{
  pub fn stage(&mut self, damage: Damage){
    ...
  }

  pub fn emit() -> Option<Frame>{
    ...
  }
}
```

## ファイル構成

- frame.rs
  - damage.rs
