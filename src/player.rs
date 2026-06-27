//! 動画プレイヤー（libmpv ラッパー）。
//!
//! mpv を `wid`（ウィンドウハンドル）に埋め込み、`vo=gpu-next` `gpu-api=d3d11` で
//! mpv 自身が D3D11 にウィンドウへ直接描画する（OpenGL は使わない）。UI 層からは
//! プロパティ get/set・loadfile・seek 等の薄いラッパーを呼ぶ。
//!
//! 設計メモ: mpv はプロセス終了まで生かす運用なので `Box::leak` で `'static` 化する。

use anyhow::{anyhow, Result};
use libmpv2::Mpv;

/// 動画プレイヤー（mpv を `wid` に D3D11 埋め込み）。
pub struct Player {
    mpv: &'static Mpv,
}

impl Player {
    /// 埋め込みモードで初期化する。OpenGL を一切作らず、mpv 自身が `wid`（ウィンドウハンドル）に
    /// 対し `vo=gpu-next` `gpu-api=d3d11` で直接描画する。
    pub fn new_embedded(wid: i64, verbose: bool) -> Result<Self> {
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("wid", wid)?;
            init.set_property("vo", "gpu-next")?;
            init.set_property("gpu-api", "d3d11")?;
            // YouTube URL の解決はアプリ側で行うので ytdl は無効化（egui 版と同じ）。
            init.set_property("ytdl", false)?;
            // ライブ HLS の起動を速く＆低遅延に: ffmpeg の HLS demuxer がライブ先頭から
            // 何セグメント遡って再生開始するかを末尾寄り(-1)にして初期バッファを減らす。
            // 併せて初回のキャッシュ充填待ちで一時停止しない（即再生開始）。
            let _ = init.set_property("demuxer-lavf-o", "live_start_index=-1");
            let _ = init.set_property("cache-pause-initial", false);
            // シーク再開を速く: 映像+別音声(audio-file)の2ストリーム構成では、シーク先が
            // demuxer キャッシュ内なら再取得せず即シークできる。キャッシュをシーク可能に強制し、
            // 前方・後方とも広く保持して「先読み済み範囲のシーク」を即時化する（range 再取得=数秒待ちを回避）。
            let _ = init.set_property("cache", "yes");
            let _ = init.set_property("demuxer-seekable-cache", "yes");
            let _ = init.set_property("demuxer-max-bytes", "256MiB");
            let _ = init.set_property("demuxer-max-back-bytes", "128MiB");
            // hwdec は mpv 既定のまま（明示設定しない）。
            if verbose {
                init.set_property("terminal", true)?;
                init.set_property("msg-level", "all=status")?;
            }
            Ok(())
        })
        .map_err(|e| anyhow!("mpv init (embedded) failed: {e}"))?;
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        Ok(Self { mpv })
    }

    // --- mpv プロパティアクセス（薄いラッパー）---

    pub fn paused(&self) -> bool {
        self.mpv.get_property("pause").unwrap_or(false)
    }
    pub fn set_paused(&self, paused: bool) {
        let _ = self.mpv.set_property("pause", paused);
    }
    pub fn time_pos(&self) -> f64 {
        self.mpv.get_property("time-pos").unwrap_or(0.0)
    }
    /// 再生対象が無くアイドル状態か。loadfile したのに idle に戻った＝そのファイルを開けなかった
    /// （403 等）の判定に使う（中継フォールバックの起動条件）。
    pub fn idle_active(&self) -> bool {
        self.mpv.get_property("idle-active").unwrap_or(false)
    }
    pub fn set_time_pos(&self, t: f64) {
        // time-pos プロパティ設定だと、YouTube の映像+別音声(audio-file)構成では実シークが
        // 効かない（プロパティ値だけ変わって映像が動かない）ことがある。mpv の seek コマンド
        // (absolute, 秒) で確実にシークする（mpv OSC や旧 probe と同じ方式）。
        let _ = self.mpv.command("seek", &[&format!("{t:.3}"), "absolute"]);
    }
    pub fn duration(&self) -> f64 {
        self.mpv.get_property("duration").unwrap_or(0.0)
    }
    /// シーク可能か（VOD / DVR ありライブ = true、DVR 無しライブ = false）。
    pub fn seekable(&self) -> bool {
        self.mpv.get_property("seekable").unwrap_or(false)
    }
    pub fn volume(&self) -> f64 {
        self.mpv.get_property("volume").unwrap_or(100.0)
    }
    pub fn set_volume(&self, v: f64) {
        let _ = self.mpv.set_property("volume", v);
    }
    pub fn muted(&self) -> bool {
        self.mpv.get_property("mute").unwrap_or(false)
    }
    pub fn set_muted(&self, m: bool) {
        let _ = self.mpv.set_property("mute", m);
    }
    /// 再生中メディアのタイトル（mpv が解決したもの）。
    pub fn media_title(&self) -> String {
        self.mpv
            .get_property::<String>("media-title")
            .unwrap_or_default()
    }
    pub fn seek_relative(&self, secs: f64) {
        let _ = self
            .mpv
            .command("seek", &[&secs.to_string(), "relative"]);
    }

    /// ライブ配信の先端（最新）へ追いつく。前方への大きな相対シークで mpv に
    /// シーク可能範囲の末尾（＝ライブ先端）までクランプさせる。DVR 窓のあるライブで有効。
    pub fn seek_to_live(&self) {
        // 大きく前へ飛ばす（ライブ先端を超えられないのでクランプされる）。keyframe ではなく
        // exact で末尾に寄せる。
        let _ = self.mpv.command("seek", &["100000", "relative+exact"]);
        // 念のため割合 100% でも寄せる（実装差吸収）。
        let _ = self.mpv.command("seek", &["100", "absolute-percent"]);
    }

    /// 再生中メディアのタイトルを上書きする（解決後に後追いで設定する用）。
    pub fn set_force_media_title(&self, title: &str) {
        let _ = self.mpv.set_property("force-media-title", title);
    }

    /// HW デコードの設定を動的に変更する。`"auto"` で HW 利用、`"no"` で SW 強制。
    /// GPU 使用率監視（gpu_usage）から、使用率が高い時に呼ばれる。
    pub fn set_hwdec(&self, mode: &str) {
        let _ = self.mpv.set_property("hwdec", mode);
    }

    /// 映像の右マージン比率（0.0..1.0）を設定する。ネイティブ版でチャットを右に出すとき、
    /// 動画を左に縮めて重なりを避けるために使う（真の左右分割）。
    pub fn set_video_margin_right(&self, ratio: f64) {
        let _ = self
            .mpv
            .set_property("video-margin-ratio-right", ratio.clamp(0.0, 0.9));
    }

    /// 動画を読み込む（loadfile replace）。
    /// `audio_url` / `title` は loadfile のオプションとして `audio-file=` / `force-media-title=` を渡す。
    pub fn loadfile(
        &self,
        video_url: &str,
        audio_url: Option<&str>,
        title: Option<&str>,
    ) -> Result<()> {
        let mut options = Vec::new();
        if let Some(a) = audio_url {
            options.push(format!("audio-file={a}"));
        }
        if let Some(t) = title {
            options.push(format!("force-media-title={t}"));
        }
        if options.is_empty() {
            self.mpv
                .command("loadfile", &[video_url])
                .map_err(|e| anyhow!("loadfile failed: {e}"))
        } else {
            let opts = options.join(",");
            self.mpv
                .command("loadfile", &[video_url, "replace", "-1", &opts])
                .map_err(|e| anyhow!("loadfile failed: {e}"))
        }
    }
}

