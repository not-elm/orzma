# OzmuxConfigs を直接 Deserialize にする設計

- 日付: 2026-06-20
- 対象クレート: `crates/ozmux_configs`（消費側 `src/font.rs` を含む）
- ステータス: 設計承認済み（実装計画はこの後）

## 背景 / 動機

設定のロードは単層である:

```
toml::from_str::<RawConfigs>()  →  RawConfigs::apply_to(OzmuxConfigs::default())  →  validate()
```

`apply_to` に渡す base は**常に `OzmuxConfigs::default()`** であり、Patch パターンが本来持つ
「任意の base にマージする」汎用性は使われていない。各セクションには resolved 型
（`Theme` / `MouseConfig` / `FontConfig` …）と、その全フィールドを `Option` でくるんだ
patch 型（`ThemePatch` / `MousePatch` / `FontPatch` …）が二重に存在し、`apply_to` の多くは
`unwrap_or(base.x)` を並べただけのボイラープレートになっている。

### 鍵となる serde の挙動

コンテナ属性 `#[serde(default)]` は、デシリアライズ時に**欠けた各フィールドをその構造体の
`Default` 実装から補完する**。つまり「部分上書き＝指定の無いフィールドは default に
フォールバック」という patch の per-field マージは、patch 型を介さずとも
`#[serde(default)]` ＋ 既存の `impl Default` だけで再現できる。

この挙動はリポジトリ内に前例がある:

- `Shortcuts` / `Bindings` は patch 型を持たず、`#[serde(default, deny_unknown_fields)]` ＋
  per-field `deserialize_with` ＋ 廃止キーの `skip_serializing` で**既に resolved 型を直接
  デシリアライズしている**。
- `startup_mode`（enum `StartupMode`）も patch を介さず直接デシリアライズされている。

## ゴール / 非ゴール

### ゴール

- `RawConfigs` と全 `*Patch` 型（`ThemePatch` / `FontPatch` / `FacePatch` / `MousePatch` /
  `KeyboardPatch` / `InactivePaneConfigPatch` / `OscWebviewPatch` / `TmuxPatch` /
  `OzmaPatch`）を廃止する。
- `OzmuxConfigs` と各セクションの resolved 型を直接 `Deserialize` にする。
- パース結果・エラー挙動を現状と一致させる（後述の意図的な破壊的変更を除く）。

### 非ゴール

- `Serialize` 導出の整理（font を除く）。旧 web frontend 由来で未使用だが、本 refactor の
  対象外とし、各 resolved 型の既存 `Serialize` 導出はそのまま残す。
- `shortcuts` / `Bindings` の構造変更。既に直接デシリアライズ済みのため**変更しない**
  （廃止キー群もそのまま維持）。
- 設定スキーマの全面見直し。変更は font セクションの flat 化のみ。

## 設計

### 1. ロードパイプライン（`lib.rs`）

`parse_and_validate` を廃止し、`load_with_env` 内に `parse → normalize → validate` を置く。

```rust
fn load_with_env(env: &dyn path::Env) -> OzmuxConfigsResult<Self> {
    let configured_path = path::resolve_config_path(env)?;
    tracing::info!(path = %configured_path.display(), "resolving ozmux config path");

    let text = match std::fs::read_to_string(&configured_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(path = %configured_path.display(), "ozmux config not found; using defaults");
            return Ok(Self::default());
        }
        Err(source) => return Err(OzmuxConfigsError::Io { path: configured_path, source }),
    };

    let mut configs: OzmuxConfigs =
        toml::from_str(&text).map_err(|source| OzmuxConfigsError::ParseToml {
            path: configured_path.clone(),
            source,
        })?;
    configs.normalize();
    configs.validate()?;
    Ok(configs)
}
```

- `normalize()` … 寛容な正規化（`inactive_pane` の clamp/NaN/hex フォールバック）。エラーを出さない。
- `validate()` … クロスセクションのハードエラー検証。失敗で `Err` を返しロードを中断。

### 2. トップレベル `OzmuxConfigs`

```rust
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct OzmuxConfigs {
    pub shortcuts: Shortcuts,
    pub theme: Theme,
    pub font: FontConfig,
    pub mouse: MouseConfig,
    pub keyboard: KeyboardConfig,
    pub inactive_pane: InactivePaneConfig,
    pub osc_webview: OscWebviewConfig,
    pub tmux: TmuxConfig,
    pub ozma: OzmaConfig,
    pub startup_mode: StartupMode,
}
```

- `#[serde(default)]` … セクション/フィールドが欠けたら各 `Default` から補完。
- `#[serde(deny_unknown_fields)]` … 旧 `RawConfigs` が担っていたセクション名タイポ検出
  （`[shortucts]` 等）を継承。

`normalize` と `validate` は `OzmuxConfigs` の **private メソッド**として実装する:

```rust
impl OzmuxConfigs {
    fn normalize(&mut self) {
        self.inactive_pane.normalize();
    }

    fn validate(&self) -> OzmuxConfigsResult<()> {
        if let Err(dupes) = self.shortcuts.bindings.validate_no_conflicts() {
            return Err(OzmuxConfigsError::DuplicateChords(dupes));
        }
        let size = self.font.size;
        if !(size > 0.0 && size <= 200.0) {
            return Err(OzmuxConfigsError::InvalidFontSize { size });
        }
        Ok(())
    }
}
```

### 3. 各セクションの変更（A＝振る舞い保存）

各 resolved 型に `Deserialize` 導出（既にあるものは流用）と `#[serde(default)]` を付け、
`impl Default` は維持する。`deny_unknown_fields` の有無はセクションごとに**現状を保存**する。

| セクション | 変更 | deny_unknown（保存） | normalize |
|---|---|---|---|
| theme | `ThemePatch` 削除、`#[serde(default)]` 追加 | なし | 不要 |
| mouse | `MousePatch` 削除（12項目の if-let 消滅）、`#[serde(default)]` | なし | 不要 |
| keyboard | `KeyboardPatch` 削除、`#[serde(default, deny_unknown_fields)]` | あり | 不要 |
| osc_webview | `OscWebviewPatch` 削除、`#[serde(default)]` | なし | 不要 |
| ozma | `OzmaPatch` 削除、`#[serde(default, deny_unknown_fields)]` | あり | 不要 |
| tmux | `TmuxPatch` 削除、`#[serde(default, deny_unknown_fields)]`、廃止 `auto_connect` を**削除** | あり | 不要 |
| inactive_pane | `InactivePaneConfigPatch` 削除、`#[serde(default)]` | なし | **あり** |
| font | `FontPatch`/`FacePatch` 削除、flat 書式へ（後述） | なし | 不要 |
| shortcuts | 変更なし | あり | — |
| startup_mode | 変更なし | — | — |

注: `osc_webview` の `enabled: bool`（default `true`）や `font.size`（default `11.25`）のような
「`Default::default()` と異なる既定値」は、コンテナ `#[serde(default)]` ＋ 既存 `impl Default`
で正しく補完される（フィールド単位の `#[serde(default = "fn")]` ヘルパは不要）。

注: `OzmaConfig` は resolved 型の中で**唯一 `Deserialize` 導出を持たない**（現状は `OzmaPatch`
側にだけある, `ozma.rs:6`）。実装時は `OzmaConfig` 自体に `#[derive(Deserialize, …)]` ＋
`#[serde(default, deny_unknown_fields)]` を追加する（`use serde::Deserialize;` は既存）。他の
resolved 型（Theme/MouseConfig/KeyboardConfig/InactivePaneConfig/OscWebviewConfig/TmuxConfig/
FontConfig）は既に `Deserialize` を導出済み。

### 4. font（flat スキーマ）

family / style を既に削除し各 face テーブルが `path` のみになったため、ネストを廃し flat 化する。

**TOML（変更後）:**

```toml
[font]
size = 14.0
normal = "~/fonts/Regular.ttf"
bold = "~/fonts/Bold.ttf"
italic = "~/fonts/Italic.ttf"
bold_italic = "~/fonts/BoldItalic.ttf"
```

**resolved 型:**

```rust
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FontConfig {
    pub size: f32,
    pub normal: Option<std::path::PathBuf>,
    pub bold: Option<std::path::PathBuf>,
    pub italic: Option<std::path::PathBuf>,
    pub bold_italic: Option<std::path::PathBuf>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self { size: 11.25, normal: None, bold: None, italic: None, bold_italic: None }
    }
}
```

- `FontPatch` / `FacePatch` は完全に削除。
- `Serialize` は導出しない（未使用であることが確認済み。これによりパスの情報漏えい対策
  `#[serde(skip_serializing)]` も不要になる）。`Default` は手書き（既定 size 11.25 のため）。
- 消費側 `src/font.rs`（約8箇所）: `font.normal_path.as_deref()` → `font.normal.as_deref()` 等の
  機械的置換。`no_override` 判定の 4 フィールドも同様。

### 5. `inactive_pane` の `normalize()`

旧 `InactivePaneConfigPatch::apply_to` が行っていた寛容な処理を、デシリアライズ後の
`normalize()` パスへ移す。`apply_unit` は `norm_unit` に置換する。

```rust
impl InactivePaneConfig {
    pub(crate) fn normalize(&mut self) {
        let d = Self::default();
        self.dim = norm_unit(self.dim, d.dim);
        self.tint = norm_unit(self.tint, d.tint);
        self.webview_dim = norm_unit(self.webview_dim, d.webview_dim);
        self.webview_desaturate = norm_unit(self.webview_desaturate, d.webview_desaturate);
        if parse_hex_rgb(&self.tint_color).is_some() {
            self.tint_color = self.tint_color.to_ascii_lowercase();
        } else {
            self.tint_color = d.tint_color;
        }
    }
}

fn norm_unit(v: f32, default: f32) -> f32 {
    if v.is_nan() { default } else { v.clamp(0.0, 1.0) }
}
```

`parse_hex_rgb` は private のまま維持（`is_ascii` ガードも load-bearing なので保持）。

**振る舞い保存の確認:**

| 入力 | 現状 | 新（deser→normalize） |
|---|---|---|
| 欠落 | base default | default 補完 → clamp（既定は範囲内なので不変） |
| `dim = 4.0` | clamp → 1.0 | 4.0 → clamp → 1.0 |
| `dim = nan` | base 維持 → 1.0 | NaN → default 1.0 |
| `tint_color = "#FF00AB"` | 小文字化 `#ff00ab` | 同 |
| `tint_color = "not-a-color"` | base 維持 `#3a3b45` | 不正 → default `#3a3b45` |
| `tint_color = "#中文"` | base 維持（is_ascii ガード） | 不正 → default |

### 6. ファイル構成の変更

- `raw.rs` を**削除**（`RawConfigs` ＋ `apply_to` ＋ `validate` ＋ そのテスト）。`validate` は
  `OzmuxConfigs::validate` メソッドへ移設。`raw.rs` 由来のクロスセクションテスト
  （`empty_raw_returns_defaults` 等）は `lib.rs` のテストへ統合し、`toml::from_str::<OzmuxConfigs>`
  を直接叩く形に書き換える。
- `lib.rs` の `mod raw;` を削除。

## 破壊的変更（意図的）

1. **font スキーマ**: 旧 `[font.normal] path = "..."`（ネストテーブル）形式は、新 `normal` フィールドが
   文字列パスを期待するため**型エラーでロード失敗**する。新形式 `normal = "<path>"` に移行が必要。
2. **tmux `auto_connect`**: 廃止キーの受理を削除。`[tmux]` に `auto_connect = …` が残っている
   config は `deny_unknown_fields` により**ロード失敗**する。

いずれもプロジェクト方針（Alacritty 互換の廃止、廃止キーの整理）に沿う。

## 振る舞い保存マトリクス（font / tmux を除く）

- `deny_unknown_fields`: トップレベル＝あり、keyboard/ozma/tmux/shortcuts＝あり、
  theme/mouse/osc_webview/inactive_pane/font＝なし（すべて現状どおり）。
- `inactive_pane` の不正値→default の黙ったフォールバック、NaN 拒否、clamp、色の小文字正規化＝
  `normalize()` で同一の結果。
- 未指定セクション/フィールド→default＝コンテナ `#[serde(default)]` で同一。

## テスト方針

- 各セクションの `*Patch` 関連テスト（`apply_to` 系）は削除し、`toml::from_str::<セクション型>`
  による直接デシリアライズ＋（必要なら）`normalize` のテストへ置換する。
- per-field フォールバック（部分セクションで未指定フィールドが default になる）を各セクションで検証。
- `deny_unknown_fields` を持つセクション（keyboard/ozma/tmux）で未知キーがエラーになることを検証。
- トップレベルでセクション名タイポ（`[shortucts]`）がエラーになることを検証。
- `inactive_pane`: 既存の clamp/NaN/hex/非ASCII テストを `deser→normalize` 経路で再現。
- font: `crates/ozmux_configs` の font テストと `src/font.rs` のテスト fixture（`[font.normal]\npath=`）を
  新 flat 書式 `[font]\nnormal=` に更新（`src/font.rs` の corrupt_bold テストは `[font.normal]`/
  `[font.bold]` の2テーブルを含むので両方変換）。
- font（破壊的変更の固定化）: 旧ネスト `[font.normal] path=` を**`OzmuxConfigs` 全体**へ
  デシリアライズしてロード失敗することを検証（`FontConfig` 単体でなくトップレベル経路で、
  将来の互換シム混入を防ぐ）。
- tmux: `auto_connect` 受理テストは削除し、代わりに `auto_connect` がエラーになることを検証。
- `lib.rs` 統合テスト: 空 TOML→全 default、部分セクション、`validate` のハードエラー
  （font.size 範囲外、chord 衝突）を網羅。
- `normalize()` / `validate()` は private だが、`lib.rs` の `#[cfg(test)] mod tests` から直接
  呼び出せる。ファイル経由のフルパイプライン検証は `test_support::load_with_overrides` ＋
  一時ファイルで行う（ユーザー決定どおり `parse_and_validate` ヘルパは設けない）。

## 影響範囲

- クレート外への影響は `src/font.rs`（font フィールド名）のみ。`*Patch` / `RawConfigs` は
  クレート内 `pub(crate)`/`pub` だが**クレート外から未参照**であることを確認済み。
