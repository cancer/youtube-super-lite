//! UI 非依存の値型。resolve が選択基準として使うため lib に置く。

/// 画質（最大の縦解像度）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Auto,
    P2160,
    P1440,
    P1080,
    P720,
    P480,
    P360,
}

impl Quality {
    pub const ALL: [Quality; 7] = [
        Quality::Auto,
        Quality::P2160,
        Quality::P1440,
        Quality::P1080,
        Quality::P720,
        Quality::P480,
        Quality::P360,
    ];
    pub fn height(self) -> Option<u32> {
        match self {
            Quality::Auto => None,
            Quality::P2160 => Some(2160),
            Quality::P1440 => Some(1440),
            Quality::P1080 => Some(1080),
            Quality::P720 => Some(720),
            Quality::P480 => Some(480),
            Quality::P360 => Some(360),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Quality::Auto => "自動",
            Quality::P2160 => "2160p",
            Quality::P1440 => "1440p",
            Quality::P1080 => "1080p",
            Quality::P720 => "720p",
            Quality::P480 => "480p",
            Quality::P360 => "360p",
        }
    }
}

/// 映像コーデック。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Auto,
    H264,
    Vp9,
    Av1,
}

impl Codec {
    pub const ALL: [Codec; 4] = [Codec::Auto, Codec::H264, Codec::Vp9, Codec::Av1];
    pub fn label(self) -> &'static str {
        match self {
            Codec::Auto => "自動",
            Codec::H264 => "H.264",
            Codec::Vp9 => "VP9",
            Codec::Av1 => "AV1",
        }
    }
}

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
