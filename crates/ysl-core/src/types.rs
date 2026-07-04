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
