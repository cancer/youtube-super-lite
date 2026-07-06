# 音声イコライザ実装ガイド — パラメトリック3ノブ EQ

対象: 音声イコライザ機能を実装する人。
このガイドは**迷ったら立ち返る場所**として書かれている。設計判断はすべて確定済みなので、
実装中に「どっちがいいか」を再検討する必要はない。判断の理由(Why)は各所に書いてある。

前提知識: Rust の基礎、このリポジトリのビルド手順(README)。
必読: **[docs/design/design-principles.md](../docs/design/design-principles.md)**（着工前に必ず読むこと）。
層構造の全体像は [docs/design/architecture-overview.md](../docs/design/architecture-overview.md) と
[issue11-implementation-guide.md](issue11-implementation-guide.md) §1 を参照。

---

## 0. これは何の機能か

トーク・ライブ配信で声を聞き取りやすくするための音声イコライザ。ノブは3つ:

1. **ボイス帯域ゲイン** — 1.8kHz 中心の peaking EQ を ±12dB（0dB = オフ）
2. **ローパスフィルタ** — カットオフ周波数を段階選択（オフ可）。高域のシャリつき・ノイズを落とす
3. **ハイパスフィルタ** — カットオフ周波数を段階選択（オフ可）。低域のこもり・ハムを落とす

実現手段は mpv の **`af`（audio filter）プロパティ**。FFmpeg のフィルタチェーン文字列を
`set_property("af", ...)` するとランタイムで適用され、空文字 `""` で解除される:

```
lavfi=[highpass=f=100,equalizer=f=1800:width_type=q:w=1.2:g=6,lowpass=f=8000]
```

- `lavfi=[...]` で明示ラップする（mpv の af 構文パーサと FFmpeg のコロン区切りの衝突を避ける）
- `width_type=q` は FFmpeg 完全名を使う（短縮形 `t=q` の曖昧性を排除）

**`af` は mpv のグローバルプロパティ**。本プロジェクトの `loadfile`（per-file オプションは
`audio-file=` / `force-media-title=` のみ。`player.rs` の `loadfile` 参照）では上書きされないため、
ファイル切替（loadfile replace）後も保持される。→ set するタイミングは
**「起動時 restore」と「パラメータ変更時」の2箇所だけ**でよい。再生開始のたびに再適用する
コードを書かないこと。

## 1. 設計の全体像

```
settings.json ─(起動時 load)─┐
                              ▼
UiAction::Eq*  ──▶ apply_action ──▶ playback::set_eq(pb, EqParams)   ← 唯一の適用点
(devtools /action/eq_*)              │      │
(PR2: オーバーレイ)                  │      └─▶ pb.eq = eq.clamped()   （状態の真実）
                              ┌──────┘
                              ▼
                    player.set_af(eq.mpv_af())   ← mpv バックエンドのレンダラ
```

### バックエンド中立が最重要の設計制約（#16 対応）

[Issue #16](https://github.com/cancer/youtube-super-lite/issues/16) で、SABR 化で詰んだライブは
公式 IFrame プレーヤー（WebView2）で再生するハイブリッド構成が確定している（現在 PoC 前）。
WebView2 経路には mpv がいないので、`af` 文字列は効かない。EQ を将来そのまま WebView2 経路にも
載せるため、次の3点を守る:

1. **`EqParams` はバックエンド中立の純データ**（dB と Hz のみ）。mpv の語彙（`af`、`lavfi`）を
   フィールド・メソッド名に持ち込まない
2. mpv 用文字列の生成は **`mpv_af()`** という名前のレンダラ（mpv 専用であることを名前で明示）。
   #16 実装時には `webaudio_*` レンダラが兄弟として並ぶ（§8 付録）
3. **適用の合流点は `playback::set_eq` ただ1箇所**。UI・actions・settings・/state は EqParams
   だけに依存し、フィルタ文字列を一切知らない。#16 でバックエンド分岐（mpv/webview）が入るのは
   `set_eq` の中だけ

> **Why**: この3点が守られていれば、#16 実装者は `set_eq` に webview の腕を1本足すだけで済む。
> どこか1箇所でも `mpv_af()` の結果を UI 層が直接触ると、その箇所の数だけ #16 の修正点が増える。

### 語彙（issue11 ガイド §1 と同じ）

- **データ構造体** = `EqParams`（状態だけ。純関数レンダラと getter は持つが、装置を触るメソッドは持たない）
- **system** = `playback::set_eq`（状態を処理する関数。触る状態を引数で宣言）
- EQ は **playback 単一ドメインで閉じる**。flows.rs には一切触れない

## 2. 絶対ルール（違反したら PR は差し戻し）

1. **1PR=1スコープ。PR1（lib層+API+永続化）と PR2（オーバーレイUI）を束ねた PR は差し戻し**。
   マージ待ちは不要 — PR2 は PR1 のブランチにスタックして先に作業してよい
2. **`EqParams` にロジックのメソッドを生やさない**。許されるのは純関数レンダラ
   （`mpv_af`）・判定/変換（`is_neutral`/`clamped`/`*_step`）・getter だけ。mpv や設定ファイルを
   触るコードを types.rs に書かない
3. **EQ 状態の書き込みは `playback::set_eq` のみ**。`Playback::eq` フィールドは private にして
   コンパイラに守らせる。`player.set_af` を `set_eq` 以外から呼ばない
4. **`state_json` の既存 JSON キー名は変更禁止**（外部 dev-tools クライアントが消費。追加のみ可）。
   settings.json も既存キー（`chat_font_px`/`chat_width_ratio`）不変更・追加のみ
5. 各 PR は `cargo check` **警告0**・`.\build.ps1` 成功・各 PR の「Done の定義」のスモークを
   通してからレビューに出す
6. **デバッグ起動時は、再生を始める前に必ず `POST /action/mute` を打つ**（検証中に音を鳴らさない）
7. **flows.rs に手を出さない**（EQ は playback 単一ドメイン。跨ぎ system ではない）
8. 「ついでに直したい」ものを見つけたら Issue 化して先に進む。挙動変更は本ガイド指定箇所のみ

## 3. 共通の検証手段

```powershell
cargo test -p ysl-core   # EqParams の純関数テスト（PR1 で新設）
cargo check              # エラー0・警告0
.\build.ps1              # ビルド（libmpv-2.dll のコピー込み）
.\target\debug\youtube-super-lite.exe --enable-dev-tools
```

dev-tools は `http://127.0.0.1:<起動ログに表示されたポート>` に立つ。使うのは:
- `GET /state` — UI 状態 JSON（EQ の現在値を確認）
- `POST /action/<name>` — UI 操作の注入
- `GET /screenshot` / `POST /click?x=&y=` — PR2 の見た目・スライダー検証

### 行番号について

本ガイドの行番号は**執筆時点（main = 7448284 ごろ）の参考値**。ズレていたら**関数名を一次キー**
として探すこと。

---

## 4. PR1 — EQ ドメイン + dev-tools API + 永続化

**目的**: UI なしで完結する EQ の本体。dev-tools だけで全機能を検証できる状態にする。

### 4.1 データモデル — `crates/ysl-core/src/types.rs`

Quality/Codec の下に追加。**このコードをそのまま使う**（値も確定済み。変えない）:

```rust
/// 音声イコライザ設定（バックエンド中立の純データ。dB と Hz のみで、
/// mpv や Web Audio の語彙を持たない — 適用は playback::set_eq が行う）。
/// 全フィールド既定＝ニュートラル（フィルタ無し）。
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct EqParams {
    /// ボイス帯域（VOICE_FREQ_HZ の peaking EQ）のゲイン dB。0.0 = オフ。-12.0..=12.0。
    pub voice_gain_db: f64,
    /// ローパスカットオフ Hz。None = オフ。
    pub lowpass_hz: Option<f64>,
    /// ハイパスカットオフ Hz。None = オフ。
    pub highpass_hz: Option<f64>,
}

/// ボイス帯域 peaking EQ の中心周波数。人声の明瞭度に効く 1〜3kHz の中心。
pub const VOICE_FREQ_HZ: f64 = 1800.0;
/// ボイス帯域 peaking EQ の Q。1.2 ≒ 1〜3kHz を緩やかに持ち上げる幅。
pub const VOICE_Q: f64 = 1.2;
/// ローパスカットオフの段階。devtools のステップ操作と UI スライダーの量子化が
/// 共有する唯一の定義（最上段 16k の1つ先＝オフ）。
pub const LOWPASS_STEPS: [f64; 9] = [
    1000.0, 1500.0, 2000.0, 3000.0, 4000.0, 6000.0, 8000.0, 12000.0, 16000.0,
];
/// ハイパスカットオフの段階（最下段 40 の1つ先＝オフ）。
pub const HIGHPASS_STEPS: [f64; 9] = [
    40.0, 60.0, 80.0, 100.0, 150.0, 200.0, 300.0, 500.0, 1000.0,
];

impl EqParams {
    /// mpv の `af` プロパティ用レンダラ。ニュートラルなら空文字（＝フィルタ解除）。
    /// 順序は highpass → equalizer → lowpass 固定。
    /// バックエンド固有の文字列はここだけで作る（UI 層に漏らさない）。
    pub fn mpv_af(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(hz) = self.highpass_hz {
            parts.push(format!("highpass=f={hz}"));
        }
        if self.voice_gain_db != 0.0 {
            parts.push(format!(
                "equalizer=f={VOICE_FREQ_HZ}:width_type=q:w={VOICE_Q}:g={}",
                self.voice_gain_db
            ));
        }
        if let Some(hz) = self.lowpass_hz {
            parts.push(format!("lowpass=f={hz}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("lavfi=[{}]", parts.join(","))
        }
    }

    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// 値域に収める。voice は ±12dB、カットオフはラダーの端にクランプ
    /// （settings.json の手編集など、外から来た値の防波堤）。
    pub fn clamped(mut self) -> Self {
        self.voice_gain_db = self.voice_gain_db.clamp(-12.0, 12.0);
        self.lowpass_hz = self
            .lowpass_hz
            .map(|v| v.clamp(LOWPASS_STEPS[0], LOWPASS_STEPS[LOWPASS_STEPS.len() - 1]));
        self.highpass_hz = self
            .highpass_hz
            .map(|v| v.clamp(HIGHPASS_STEPS[0], HIGHPASS_STEPS[HIGHPASS_STEPS.len() - 1]));
        self
    }

    /// ローパスをラダー上で ±1 段動かす。オフから -1 で最上段（16k、一番弱い）から入り、
    /// 最上段から +1 でオフに抜ける。下端では止まる。
    pub fn lowpass_step(cur: Option<f64>, dir: i32) -> Option<f64> {
        match cur {
            None if dir < 0 => Some(LOWPASS_STEPS[LOWPASS_STEPS.len() - 1]),
            None => None,
            Some(v) => {
                let idx = nearest_idx(&LOWPASS_STEPS, v) as i32 + dir;
                if idx >= LOWPASS_STEPS.len() as i32 {
                    None // 最上段の先＝オフ
                } else {
                    Some(LOWPASS_STEPS[idx.max(0) as usize])
                }
            }
        }
    }

    /// ハイパスをラダー上で ±1 段動かす。オフから +1 で最下段（40Hz、一番弱い）から入り、
    /// 最下段から -1 でオフに抜ける。上端では止まる。
    pub fn highpass_step(cur: Option<f64>, dir: i32) -> Option<f64> {
        match cur {
            None if dir > 0 => Some(HIGHPASS_STEPS[0]),
            None => None,
            Some(v) => {
                let idx = nearest_idx(&HIGHPASS_STEPS, v) as i32 + dir;
                if idx < 0 {
                    None // 最下段の先＝オフ
                } else {
                    Some(HIGHPASS_STEPS[(idx as usize).min(HIGHPASS_STEPS.len() - 1)])
                }
            }
        }
    }
}

/// v に最も近いラダー段の index。
fn nearest_idx(steps: &[f64], v: f64) -> usize {
    let mut best = 0;
    for (i, s) in steps.iter().enumerate() {
        if (v - s).abs() < (v - steps[best]).abs() {
            best = i;
        }
    }
    best
}
```

同ファイル末尾にテストを置く（**このリポジトリ初のユニットテスト**。`cargo test -p ysl-core` で回る）:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpv_af_neutral_is_empty() {
        assert_eq!(EqParams::default().mpv_af(), "");
    }

    #[test]
    fn mpv_af_voice_only() {
        let eq = EqParams { voice_gain_db: 6.0, ..Default::default() };
        assert_eq!(eq.mpv_af(), "lavfi=[equalizer=f=1800:width_type=q:w=1.2:g=6]");
    }

    #[test]
    fn mpv_af_full_chain_order() {
        let eq = EqParams {
            voice_gain_db: -3.0,
            lowpass_hz: Some(8000.0),
            highpass_hz: Some(100.0),
        };
        assert_eq!(
            eq.mpv_af(),
            "lavfi=[highpass=f=100,equalizer=f=1800:width_type=q:w=1.2:g=-3,lowpass=f=8000]"
        );
    }

    #[test]
    fn lowpass_step_off_crossings() {
        assert_eq!(EqParams::lowpass_step(None, -1), Some(16000.0)); // オフ→最上段
        assert_eq!(EqParams::lowpass_step(None, 1), None);           // オフのまま
        assert_eq!(EqParams::lowpass_step(Some(16000.0), 1), None);  // 最上段の先＝オフ
        assert_eq!(EqParams::lowpass_step(Some(1000.0), -1), Some(1000.0)); // 下端で止まる
    }

    #[test]
    fn highpass_step_off_crossings() {
        assert_eq!(EqParams::highpass_step(None, 1), Some(40.0));
        assert_eq!(EqParams::highpass_step(None, -1), None);
        assert_eq!(EqParams::highpass_step(Some(40.0), -1), None);
        assert_eq!(EqParams::highpass_step(Some(1000.0), 1), Some(1000.0)); // 上端で止まる
    }

    #[test]
    fn clamped_limits() {
        let eq = EqParams {
            voice_gain_db: 99.0,
            lowpass_hz: Some(1.0),
            highpass_hz: Some(99999.0),
        }
        .clamped();
        assert_eq!(eq.voice_gain_db, 12.0);
        assert_eq!(eq.lowpass_hz, Some(1000.0));
        assert_eq!(eq.highpass_hz, Some(1000.0));
    }
}
```

> 注意: `EqParams` は f64 を含むので `Eq` は derive **しない**（`PartialEq` のみ。Quality/Codec とは違う）。

### 4.2 mpv ラッパー — `crates/ysl-core/src/player.rs`

`set_hwdec`（行121-123）の直後に、同じ「いつでも set できる薄いラッパー」パターンで追加:

```rust
/// 音声フィルタチェーン（mpv `af`）を設定する。空文字で解除。
/// af はグローバルプロパティなので、再生前でも再生中でも有効で、loadfile では消えない。
pub fn set_af(&self, af: &str) {
    let _ = self.mpv.set_property("af", af);
}
```

### 4.3 playback ドメイン — `crates/ysl-core/src/playback.rs`

1. use に `EqParams` を追加: `use crate::types::{Codec, EqParams, Quality};`
2. `Playback` struct（行15-29）の「装置と好み（アプリ寿命）」ブロック、`codec: Codec` の隣に:
   ```rust
   eq: EqParams,
   ```
3. `Playback::new`（行45-58）の初期化に `eq: EqParams::default(),`
4. getter を `codec()`（行78-80）の並びに:
   ```rust
   pub fn eq(&self) -> EqParams {
       self.eq
   }
   ```
5. system 関数を `set_codec`（行97-100）の並びに:
   ```rust
   /// system: EQ 設定を変更し、再生バックエンドへ即時反映する（クランプ込み）。
   /// バックエンド分岐（#16: mpv / webview）を将来足すのはこの関数の中だけ。
   pub fn set_eq(pb: &mut Playback, eq: EqParams) {
       pb.eq = eq.clamped();
       pb.player.set_af(&pb.eq.mpv_af());
   }
   ```

### 4.4 入力合流点 — `src/ui/actions.rs`

1. `UiAction` enum（`VolumeBy` の並び）に追加。`SetEq*`/`ToggleEqPanel` は PR2 で使い始めるが
   **enum 定義は PR1 で入れてよい**（devtools 名を PR1 で予約しないこと。腕がない variant は
   まだ作らず、PR1 では下の5つだけ）:
   ```rust
   /// EQ: ボイス帯域ゲインを dB で相対変更。
   EqVoiceBy(f64),
   /// EQ: ローパスカットオフをラダー±1段（+1=カットオフを上げる→最上段の先でオフ）。
   EqLowpassStep(i32),
   /// EQ: ハイパスカットオフをラダー±1段（+1=カットオフを上げる。-1 で最下段の先はオフ）。
   EqHighpassStep(i32),
   /// EQ: 全ニュートラル（フィルタ解除）。
   EqOff,
   ```
2. `apply_action()` の match、`VolumeBy`（行362-365）の並びに追加。全腕が
   「現 eq を読む → 1フィールド差し替え → `playback::set_eq`」の3行パターン
   （戻り値・再描画などの周辺処理は `VolumeBy` の腕の形にそのまま合わせる）:
   ```rust
   UiAction::EqVoiceBy(d) => {
       let mut eq = self.playback.eq();
       eq.voice_gain_db += d;
       playback::set_eq(&mut self.playback, eq);
   }
   UiAction::EqLowpassStep(dir) => {
       let mut eq = self.playback.eq();
       eq.lowpass_hz = EqParams::lowpass_step(eq.lowpass_hz, dir);
       playback::set_eq(&mut self.playback, eq);
   }
   UiAction::EqHighpassStep(dir) => {
       let mut eq = self.playback.eq();
       eq.highpass_hz = EqParams::highpass_step(eq.highpass_hz, dir);
       playback::set_eq(&mut self.playback, eq);
   }
   UiAction::EqOff => {
       playback::set_eq(&mut self.playback, EqParams::default());
   }
   ```
3. `devtools_action()`（行273-307）の match、`"mute"`（行281）の後に追加。
   既存の「引数なし・固定ステップ」方式（vol_up=+5 と同じ）に合わせる。**名前は確定値**:
   ```rust
   "eq_voice_up" => UiAction::EqVoiceBy(1.0),
   "eq_voice_down" => UiAction::EqVoiceBy(-1.0),
   "eq_lowpass_up" => UiAction::EqLowpassStep(1),
   "eq_lowpass_down" => UiAction::EqLowpassStep(-1),
   "eq_highpass_up" => UiAction::EqHighpassStep(1),
   "eq_highpass_down" => UiAction::EqHighpassStep(-1),
   "eq_off" => UiAction::EqOff,
   ```

### 4.5 永続化 — `src/settings.rs`

**役割の拡張**: settings.rs は現在 UI 状態（chat_font_px/chat_width_ratio）専用だが、EQ は
ここに入る最初の「再生の好み」。モジュール doc コメント（行1）を
「ユーザー設定の永続化（UI 状態と再生の好み）」に更新する。ファイルは settings.json のまま1つ。
永続化は bin 層の責務で、lib 層（EqParams）は永続化を知らない。

```rust
#[derive(Clone, Copy)]
pub struct Settings {
    pub chat_font_px: f32,
    pub chat_width_ratio: f32,
    pub eq_voice_gain_db: f64,
    pub eq_lowpass_hz: Option<f64>,
    pub eq_highpass_hz: Option<f64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            chat_font_px: 16.0,
            chat_width_ratio: 0.28,
            eq_voice_gain_db: 0.0,
            eq_lowpass_hz: None,
            eq_highpass_hz: None,
        }
    }
}

impl Settings {
    /// EQ 部分を lib 層の値型として取り出す（読み取り専用の変換 getter）。
    pub fn eq_params(&self) -> ysl_core::types::EqParams {
        ysl_core::types::EqParams {
            voice_gain_db: self.eq_voice_gain_db,
            lowpass_hz: self.eq_lowpass_hz,
            highpass_hz: self.eq_highpass_hz,
        }
    }
}
```

`load()`（行29-42）に追加。**クランプは `EqParams::clamped()` に委譲**（値域の定義を
settings.rs に二重化しない）:

```rust
if let Some(g) = v["eq_voice_gain_db"].as_f64() {
    s.eq_voice_gain_db = g;
}
s.eq_lowpass_hz = v["eq_lowpass_hz"].as_f64();   // 欠落/null → None（オフ）
s.eq_highpass_hz = v["eq_highpass_hz"].as_f64();
let eq = s.eq_params().clamped();
s.eq_voice_gain_db = eq.voice_gain_db;
s.eq_lowpass_hz = eq.lowpass_hz;
s.eq_highpass_hz = eq.highpass_hz;
```

`save()`（行50-53 の `json!`）にキー追加（`Option` は None→null で出る）:

```rust
"eq_voice_gain_db": s.eq_voice_gain_db,
"eq_lowpass_hz": s.eq_lowpass_hz,
"eq_highpass_hz": s.eq_highpass_hz,
```

### 4.6 起動時 restore と save トリガ — `src/ui/shell.rs`

**save 経路を新設しない**。既存の `maybe_save_settings`（行345-361:
毎フレーム差分チェック＋800ms デバウンス、CloseRequested で force）に乗せるだけ。

1. **起動時 restore**: `init()` 内 `let settings = crate::settings::load();`（行201）の直後、
   `Ok(NativeRunning {` の前に（`playback_state` はこの時点でまだ move されていない）:
   ```rust
   // 前回の EQ 設定を mpv に反映（af はグローバルプロパティなので再生開始前でも有効）。
   playback::set_eq(&mut playback_state, settings.eq_params());
   ```
2. **save**: `maybe_save_settings` の `cur` 組み立て（行346-349）と `changed` 判定（行350-351）を:
   ```rust
   let eq = self.playback.eq();
   let cur = crate::settings::Settings {
       chat_font_px: self.chat_font_px,
       chat_width_ratio: self.chat_width_ratio,
       eq_voice_gain_db: eq.voice_gain_db,
       eq_lowpass_hz: eq.lowpass_hz,
       eq_highpass_hz: eq.highpass_hz,
   };
   let changed = cur.chat_font_px != self.saved_settings.chat_font_px
       || cur.chat_width_ratio != self.saved_settings.chat_width_ratio
       || cur.eq_voice_gain_db != self.saved_settings.eq_voice_gain_db
       || cur.eq_lowpass_hz != self.saved_settings.eq_lowpass_hz
       || cur.eq_highpass_hz != self.saved_settings.eq_highpass_hz;
   ```
   （関数 doc コメント「文字サイズ・チャット幅に変更があれば保存」も「〜と EQ」に更新）
3. **委譲 getter**: `impl NativeRunning` の `codec()`（行260-262）の並びに:
   ```rust
   pub(super) fn eq(&self) -> ysl_core::types::EqParams {
       self.playback.eq()
   }
   ```

### 4.7 /state 公開 — `src/ui/present.rs`

`state_json`（行143〜）の `"muted"`（行184）の並びに追加。**キー名は確定値**（追加のみ。
既存キーに触らない）:

```rust
"eq_voice_gain_db": self.eq().voice_gain_db,
"eq_lowpass_hz": self.eq().lowpass_hz,     // None → null
"eq_highpass_hz": self.eq().highpass_hz,
```

### 4.8 ドキュメント — `src/devtools.rs`

ルーティングは `/action/<name>` の名前をそのまま `devtools_action` に流すので**コード変更不要**。
モジュール doc（行14-22）のアクション一覧に1行追加:

```
//! - EQ: `eq_voice_up`, `eq_voice_down`, `eq_lowpass_up`, `eq_lowpass_down`,
//!   `eq_highpass_up`, `eq_highpass_down`, `eq_off`
```

### 4.9 PR1 — Done の定義

```powershell
cargo test -p ysl-core   # 全テスト green
cargo check              # 警告0
.\build.ps1
.\target\debug\youtube-super-lite.exe --enable-dev-tools
```

起動ログのポートを `$PORT` として（**手順1を必ず最初に**）:

1. `curl -X POST http://127.0.0.1:$PORT/action/mute` — **再生前ミュート（絶対ルール6）**
2. `curl http://127.0.0.1:$PORT/state` → `eq_voice_gain_db: 0.0` / `eq_lowpass_hz: null` /
   `eq_highpass_hz: null` / `muted: true`
3. `eq_voice_up` を6回 → `/state` で `eq_voice_gain_db: 6.0`
4. `eq_lowpass_down` 1回 → `eq_lowpass_hz: 16000.0`、`eq_highpass_up` 1回 → `eq_highpass_hz: 40.0`
5. 動画を再生し、**別の動画に切り替え** → `/state` で eq 値が維持されている
   （af がグローバルであることの確認）。コンソールに mpv の af エラーが出ていない
6. `eq_voice_up` を20回 → `eq_voice_gain_db: 12.0` で頭打ち（クランプ確認）
7. `eq_off` → 3値とも 0/null
8. 値を適当に変えてアプリ終了 → `%APPDATA%\YouTubeSuperLite\settings.json` に `eq_*` 3キーが
   あり、**既存2キーが無変更** → 再起動 → `/state` で復元されている
9. `/state` の**既存キー集合が変更前と同一**（追加3キー以外の差分なし）

---

## 5. PR2 — オーバーレイ UI（PR1 にスタック）

**目的**: コントローラ帯に「EQ」トグルボタンを足し、下帯の直上に小パネル
（スライダー3本＋リセット）を出す。**既存 Volume スライダーの実装を全面流用**し、
新しい描画・ヒットテスト機構を発明しない。

対象: `src/dcomp_overlay.rs`（主）、`src/ui/actions.rs`・`src/ui/shell.rs`・`src/ui/present.rs`（配線）。
行番号はズレやすいので**型名・関数名で探す**こと。

### 5.1 dcomp_overlay.rs

1. `OverlayAction` enum に追加:
   `ToggleEq` / `SetEqVoice(f64)` / `SetEqLowpass(Option<f64>)` / `SetEqHighpass(Option<f64>)` / `EqReset`
2. バンド識別子を新設:
   ```rust
   /// EQ パネルのスライダー3本を識別する。
   #[derive(Clone, Copy, PartialEq)]
   pub enum EqBand { Voice, Low, High }
   ```
3. `Drag` enum に `Eq(EqBand)` を追加（`Drag::Vol` と同型「ドラッグ中は毎 MOUSEMOVE で
   値アクションを積む」連続更新）
4. `Control` enum に variant 追加:
   ```rust
   /// EQ スライダー（ラベル付き。Volume と同じトラック＋つまみ描画・ドラッグ挙動）。
   EqSlider { rect: RECT, frac: f32, band: EqBand, label: String },
   ```
   - `rect()` の match に1腕追加
   - `press()`: `Control::Volume` の腕と同パターンで
     `Hit::Drag(Drag::Eq(*band), eq_action_from_frac(*band, frac_x(rect, x)))`
     （クリック位置の値が即入る＝Volume と同じ）
   - `draw()`: `Control::Volume` のトラック＋つまみ描画を流用し、rect の左にラベル文字
     （`Control::Time` と同じ縦センタリング）
5. **frac↔値の変換は純関数**で追加（`frac_x` の近く）。ラダー定数は
   `ysl_core::types::{LOWPASS_STEPS, HIGHPASS_STEPS}` を参照し、**devtools のステップ操作と
   同じ離散値**に量子化する:
   - Voice: 線形 `-12..=+12`、1dB 丸め。`frac = (db + 12.0) / 24.0`
   - Lowpass: frac を **10 区分**に量子化。区分 0..=8 = `LOWPASS_STEPS[i]`、**最右区分 9 = オフ(None)**
   - Highpass: 左右逆。**最左区分 0 = オフ(None)**、区分 1..=9 = `HIGHPASS_STEPS[i-1]`
   - `eq_action_from_frac(band, frac) -> OverlayAction` と、表示用の逆変換
     `eq_frac(band, EqParams) -> f32` の対で書く
6. `PlaybackView` に追加: `eq_open: bool`, `eq_voice_gain_db: f64`,
   `eq_lowpass_hz: Option<f64>`, `eq_highpass_hz: Option<f64>`
7. `WndState` に `eq_panel: RECT` を追加（ヒットテスト用。毎フレーム render で更新）
8. `build_controls`: 右フロー、音量バーとミュートボタンの間に EQ トグルボタン
   （画質/コーデックボタンと同じ `measure`→`row()` パターン）。
   ラベルは `"EQ"`、色は **EQ が有効（非ニュートラル）またはパネル開時 `ds::ACCENT_BRAND`、
   それ以外は通常 fg**
9. `render`: `active && view.eq_open && !view.list_open` のとき、下帯の直上・右寄せに
   パネルを組み立てる（右端 `cw-14`、下端 `ch-BOTTOM_H-8`、幅 ~360px、高さ スライダー3行＋
   リセット1行＋padding）:
   - 背景は下帯と同じ `p.fill_rect(..., ds::alpha(ds::BG_CANVAS, 0.72))`
   - `EqSlider`×3 と `Button { label: "リセット", action: OverlayAction::EqReset }` を
     `controls` に push（既存のヒットテストループがそのまま拾う）
   - ラベル表示例: `声 +6dB` / `声 0dB`、ローパス行は `高域カット 8kHz` / `高域カット オフ`、
     ハイパス行は `低域カット 100Hz` / `低域カット オフ`
     （ローパス＝高域を削る、ハイパス＝低域を削る。ラベルは「何を削るか」で表記する）
   - `self.state.eq_panel = eq_panel;` を state 反映箇所に追加
10. wndproc:
    - `WM_MOUSEMOVE` のドラッグ match に
      `Drag::Eq(band) => s.actions.push(eq_action_from_frac(band, frac_x(&s.drag_rect, lo)))`
    - `WM_LBUTTONDOWN` の「動画域クリック＝pause」判定に `!in_rect(&s.eq_panel, lo, hi)` を
      追加（パネルの余白クリックで再生が止まらないように）

### 5.2 配線（actions / shell / present）

1. `src/ui/actions.rs`:
   - `UiAction` に追加: `SetEqVoice(f64)` / `SetEqLowpass(Option<f64>)` /
     `SetEqHighpass(Option<f64>)` / `ToggleEqPanel`
   - `apply_action`: `SetEq*` は PR1 と同じ3行パターン（フィールドを絶対値で差し替え→`set_eq`）。
     `ToggleEqPanel` は `self.eq_open = !self.eq_open;`
   - `From<OverlayAction>` に対応追加: `ToggleEq→ToggleEqPanel`, `SetEqVoice→SetEqVoice`,
     `SetEqLowpass→SetEqLowpass`, `SetEqHighpass→SetEqHighpass`, `EqReset→EqOff`
   - `devtools_action` に `"eq_toggle" => UiAction::ToggleEqPanel,` を追加
2. `src/ui/shell.rs`:
   - `NativeRunning` に `eq_open: bool` フィールド（初期化は `chat_open: false` の並びに
     `eq_open: false`）
   - `PlaybackView` 構築箇所に `eq_open` + eq 3値（`self.playback.eq()` から）を渡す
   - オーバーレイ自動非表示の `active` 判定に `|| self.eq_open` を追加
     （**パネルを開いている間は3秒無操作でも帯を消さない**。chat_open/list_open と同じ例外扱い）
3. `src/ui/present.rs`: `state_json` に `"eq_open": self.eq_open,` を追加
4. `src/devtools.rs`: doc 一覧に `eq_toggle` を追記

### 5.3 PR2 — Done の定義

起動 → **`POST /action/mute`** → 動画を再生してから:

1. `POST /action/eq_toggle` → `/state` で `eq_open: true` → `GET /screenshot` で
   下帯直上・右寄せにパネル（スライダー3本＋リセット）が見える
2. スクショの座標を基にスライダー中央へ `POST /click?x=&y=` → `/state` の該当値が
   クリック位置に対応する離散値へ変化（lowpass/highpass はラダー値のいずれかに一致すること）
3. ボイススライダーの右端クリック → `eq_voice_gain_db: 12.0`
4. 「リセット」クリック → 3値とも 0/null
5. EQ を有効にした状態で EQ ボタンがアクセント色になっている（スクショ確認）
6. パネルの余白（スライダー外）をクリック → `/state` の `paused` が変化しない
7. パネルを開いたまま3秒放置 → オーバーレイが消えない（スクショ確認）
8. `eq_toggle` で閉じる → `eq_open: false`、通常の自動非表示に戻る

---

## 6. #16（WebView2 経路）との関係 — このガイドでは実装しない

- **やること**: §1 のバックエンド中立3点を守る。それだけ
- **やらないこと**: WebView2・Web Audio 関連のコードを書かない（#16 は PoC 前。実装は #16 側）
- #16 実装者への引き継ぎは Issue #16 のチェックリストに記載済み（`set_eq` に webview の腕を
  足し、下記付録の JS を注入する）

### 付録: WebView2 側レンダラの設計メモ（#16 実装時に使う。今は書かない）

YouTube プレーヤーは MSE（`blob:` MediaSource）で再生するため、`createMediaElementSource(video)`
が CORS 消音にならない（ブラウザの EQ 拡張と同じ手法が成立する）。`ExecuteScriptAsync` で
以下を冪等注入し、パラメータ更新はノード値の書き換えだけ（チェーン再構築不要）:

```js
(() => {
  const v = document.querySelector('video');
  if (!v) return;
  if (!window.__ysl_eq) {
    const ctx = new AudioContext();
    const src = ctx.createMediaElementSource(v);
    const hp = ctx.createBiquadFilter(); hp.type = 'highpass';
    const pk = ctx.createBiquadFilter(); pk.type = 'peaking';
    pk.frequency.value = 1800; pk.Q.value = 1.2;   // types.rs の VOICE_FREQ_HZ / VOICE_Q と同値
    const lp = ctx.createBiquadFilter(); lp.type = 'lowpass';
    src.connect(hp); hp.connect(pk); pk.connect(lp); lp.connect(ctx.destination);
    window.__ysl_eq = { hp, pk, lp };
  }
  const eq = window.__ysl_eq;
  eq.hp.frequency.value = /*highpass_hz、オフ時*/ 10;
  eq.pk.gain.value = /*voice_gain_db*/ 0;
  eq.lp.frequency.value = /*lowpass_hz、オフ時*/ 24000;
})();
```

- オフのバンドは frequency を可聴域の端（highpass=10Hz / lowpass=24kHz）に置いてバイパス相当にする
- embed をトップドキュメントとしてロードする構成ならトップの `ExecuteScriptAsync` で届く。
  ホストページ内 iframe 構成なら `CoreWebView2Frame::ExecuteScriptAsync` を使う
- この JS の生成は `EqParams` の `webaudio_*` レンダラ（`mpv_af` の兄弟）として types.rs に置く
