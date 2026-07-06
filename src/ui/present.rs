//! 状態→描画データへの純関数群。地雷（Win32/winit）に触れない「普通のコード」で、
//! dev-tools の `/state` とオーバーレイのカード描画がここに合流する（Issue #11 PR U）。

use super::shell::{ListSource, NativeRunning};

impl NativeRunning {
    /// チャンネル名から解決済みアバター URL を引く（未解決なら空＝プレースホルダ円）。
    pub(super) fn avatar_for(&self, channel: &str) -> String {
        self.avatars.url_for(channel).unwrap_or_default().to_string()
    }

    /// 現在の一覧ソースの取得が進行中か（空一覧の「取得中…」表示と /state 用）。
    pub(super) fn list_busy(&self) -> bool {
        match self.list_source {
            ListSource::Recommend => self.recommend.is_busy(),
            ListSource::Subs => self.subs.is_busy(),
            ListSource::History => self.history.is_busy(),
            ListSource::Playlist => self.playlist.is_busy(),
            ListSource::Channel => self.channel_view.is_busy(),
        }
    }

    /// 現在の一覧ソースの (ヘッダ, カード配列) を返す。
    ///
    /// カードの title/channel/thumb/id は現行データ源から常に埋まる。avatar/duration/live/meta/
    /// verified は `recommend::VideoItem`（おすすめ）では常に埋まるが、subs/history はまだ
    /// パース未対応で既定値のまま。
    pub(super) fn list_rows(&self) -> (String, Vec<crate::dcomp_overlay::Card>) {
        use crate::dcomp_overlay::Card;
        let nav = "  （1新着 2おすすめ 3履歴 4リスト / ↑↓ 選択 / Enter 決定 / Backspace 戻る / Tab・Esc 閉じる）";
        let video_card = |title: &str, channel: &str, thumb: String, id: &str| Card {
            id: id.to_string(),
            title: title.to_string(),
            channel: channel.to_string(),
            thumb,
            ..Card::default()
        };
        let (base, items): (String, Vec<Card>) = match self.list_source {
            ListSource::Subs => (
                "登録チャンネルの新着".to_string(),
                self
                    .subs
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        menu: v.menu.clone(),
                        ..Card::default()
                    })
                    .collect(),
            ),
            ListSource::Recommend => (
                "おすすめ".to_string(),
                self
                    .recommend
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        verified: v.verified,
                        menu: v.menu.clone(),
                    })
                    .collect(),
            ),
            ListSource::History => (
                "再生履歴".to_string(),
                self
                    .history
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        menu: v.menu.clone(),
                        ..Card::default()
                    })
                    .collect(),
            ),
            ListSource::Channel => (
                format!("{} の動画", self.channel_view.title()),
                self
                    .channel_view
                    .items()
                    .iter()
                    .map(|v| Card {
                        id: v.video_id.clone(),
                        title: v.title.clone(),
                        channel: v.channel.clone(),
                        thumb: v.thumbnail.clone(),
                        avatar: self.avatar_for(&v.channel),
                        duration: v.duration,
                        live: v.live,
                        meta: v.meta.clone(),
                        menu: v.menu.clone(),
                        ..Card::default()
                    })
                    .collect(),
            ),
            ListSource::Playlist => {
                if self.playlist.is_items_view() {
                    // 2 階層目: 選択した再生リストの中身（動画）。
                    let rows = self
                        .playlist
                        .items()
                        .iter()
                        .map(|v| video_card(&v.title, &v.channel, String::new(), &v.video_id))
                        .collect();
                    (
                        format!("再生リスト: {}", self.playlist.items_title()),
                        rows,
                    )
                } else {
                    // 1 階層目: 再生リスト一覧（Enter で中身を開く）。件数を meta/channel に。
                    let rows = self
                        .playlist
                        .lists()
                        .iter()
                        .map(|p| Card {
                            id: p.playlist_id.clone(),
                            title: p.title.clone(),
                            channel: format!("{} 件", p.item_count),
                            meta: Some(format!("{} 件", p.item_count)),
                            ..Card::default()
                        })
                        .collect();
                    ("再生リスト".to_string(), rows)
                }
            }
        };
        (format!("{base}{nav}"), items)
    }

    /// 現在の UI 状態を JSON 文字列で返す（dev-tools の /state 用）。
    pub(super) fn state_json(&self) -> String {
        let p = self.player();
        let source = match self.list_source {
            ListSource::Subs => "subs",
            ListSource::Recommend => "recommend",
            ListSource::History => "history",
            ListSource::Playlist => "playlist",
            ListSource::Channel => "channel",
        };
        let logged_in = self.account.channel_name().is_some_and(|c| !c.is_empty());
        let overlay_visible = {
            #[cfg(windows)]
            {
                self.overlay_visible
            }
            #[cfg(not(windows))]
            {
                false
            }
        };
        // z-order 検証用（入力は動かさない読み取り専用）: オーバーレイが実マウス入力を
        // 受けられる位置（兄弟の最前面）にあるか。
        let overlay_is_topmost: Option<bool> = {
            #[cfg(windows)]
            {
                self.dcomp_overlay.as_ref().map(|o| o.is_topmost())
            }
            #[cfg(not(windows))]
            {
                None
            }
        };
        serde_json::json!({
            "current_url": self.current_url(),
            "url_input": self.url_input,
            "paused": p.paused(),
            "time_pos": p.time_pos(),
            "duration": p.duration(),
            "seekable": p.seekable(),
            "is_live": self.is_live(),
            "volume": p.volume(),
            "muted": p.muted(),
            "eq_voice_gain_db": self.eq().voice_gain_db,
            "eq_lowpass_hz": self.eq().lowpass_hz,     // None → null
            "eq_highpass_hz": self.eq().highpass_hz,
            "media_title": p.media_title(),
            "quality": self.quality().label(),
            "codec": self.codec().label(),
            "chat_open": self.chat_open,
            "chat_font_px": self.chat_font_px,
            "chat_width_ratio": self.chat_width_ratio,
            "chat_scroll": self.chat_scroll,
            "chat_available": self.chat.as_ref().is_some_and(|c| c.available()),
            "chat_messages": self.chat.as_ref().map_or(0, |c| c.messages().len()),
            "list_open": self.list_open,
            "list_source": source,
            "list_sel": self.list_sel,
            "list_count": self.list_rows().1.len(),
            "list_busy": self.list_busy(),
            "card_menu_open": self.card_menu_open,
            "overlay_is_topmost": overlay_is_topmost,
            "logged_in": logged_in,
            "channel": self.account.channel_name(),
            "auth_status": self.account.status(),
            "focused": self.focused,
            "overlay_visible": overlay_visible,
        })
        .to_string()
    }
}
