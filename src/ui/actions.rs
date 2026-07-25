//! 全入力（オーバーレイ/dev-tools/キーボード）の合流点。`apply_action` が唯一のエントリ
//! ポイントで、3 系統はいずれもここで組み立てた行動を渡すだけにする（Issue #11 PR B/U）。
//! last_activity 等の跨ぎ状態には触れず、「操作があったか」を戻り値で返すだけにする（§9.2。
//! last_activity の実際の更新は shell だけが行う）。

use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};

use ysl_core::types::EqParams;
use ysl_core::yt::{auth, resolve};
use ysl_core::{account, chat, content, flows, playback};
use crate::{Codec, Quality};

#[cfg(windows)]
use super::shell::PlaybackMode;
use super::shell::{ListSource, NativeRunning};

/// 全入力系統（オーバーレイ/dev-tools/キーボード）が組み立てて `apply_action` に渡す行動。
/// 「同一アクションの実装が1箇所ずつ」を実現する唯一のエントリポイント（Issue #11 PR B）。
pub(super) enum UiAction {
    TogglePause,
    /// シーク（0.0..=1.0 の割合。seekable 時のみ。シークバードラッグ用）。
    SeekTo(f64),
    /// シーク（相対、秒。± の量。キーボード/dev-tools の早送り/早戻し用）。
    SeekBy(f64),
    SetVolume(f64),
    VolumeBy(f64),
    ToggleMute,
    LiveEdge,
    CycleQuality,
    CycleCodec,
    Login,
    Like,
    /// URL 欄の内容を再生する（devtools の play_url・キーボードの Enter で使う）。
    PlayUrl(String),
    /// 一覧の行クリック → その video_id を再生する（座席番号(index)ではなく実 ID）。
    Play { video_id: String },
    /// カードのアバター/チャンネル名クリック → 実 channelId（無ければ名前検索）でチャンネルを開く。
    OpenChannel { id: Option<String>, name: String },
    OpenList(ListSource),
    CloseList,
    ToggleList,
    /// 一覧の選択位置を相対移動する（±1、またはグリッドの列数）。
    ListMove { delta: i32 },
    /// 現在の選択行を再生/ドリルする（devtools の list_select・キーボードの Enter）。
    ListSelect,
    /// 一覧の階層/ビューを戻る（チャンネルビュー→おすすめ、再生リスト中身→一覧）。
    ListBack,
    ToggleChat,
    ChatScroll(i32),
    /// チャット文字サイズを相対変更する（± の px）。
    ChatFontBy(f32),
    /// チャット欄の幅を絶対設定する（ウィンドウ幅比 0.15..=0.6。左端ドラッグ用）。
    SetChatWidth(f32),
    /// チャット欄の幅を相対変更する（± の比率。dev-tools の chat_wider/narrower 用）。
    ChatWidthBy(f32),
    SaveWatchLater { video_id: String },
    /// feedbackToken の送信（興味なし／チャンネルをおすすめに表示しない、いずれも同じ処理）。
    Feedback { token: String },
    OpenCardMenu(usize),
    CloseCardMenu,
    /// EQ: ボイス帯域ゲインを dB で相対変更。
    EqVoiceBy(f64),
    /// EQ: ローパスカットオフをラダー±1段（+1=カットオフを上げる→最上段の先でオフ）。
    EqLowpassStep(i32),
    /// EQ: ハイパスカットオフをラダー±1段（+1=カットオフを上げる。-1 で最下段の先はオフ）。
    EqHighpassStep(i32),
    /// EQ: 全ニュートラル（フィルタ解除）。
    EqOff,
    /// EQ: ボイス帯域ゲインを dB で絶対設定（オーバーレイのスライダードラッグ用）。
    SetEqVoice(f64),
    /// EQ: ローパスカットオフを絶対設定（オーバーレイのスライダードラッグ用。None=オフ）。
    SetEqLowpass(Option<f64>),
    /// EQ: ハイパスカットオフを絶対設定（オーバーレイのスライダードラッグ用。None=オフ）。
    SetEqHighpass(Option<f64>),
    /// EQ パネルの表示トグル。
    ToggleEqPanel,
}

impl From<crate::dcomp_overlay::OverlayAction> for UiAction {
    fn from(a: crate::dcomp_overlay::OverlayAction) -> Self {
        use crate::dcomp_overlay::{ListTab, OverlayAction};
        match a {
            OverlayAction::TogglePause => UiAction::TogglePause,
            OverlayAction::Seek(frac) => UiAction::SeekTo(frac),
            OverlayAction::SetVolume(v) => UiAction::SetVolume(v),
            OverlayAction::VolumeStep(d) => UiAction::VolumeBy(d),
            OverlayAction::ToggleMute => UiAction::ToggleMute,
            OverlayAction::LiveEdge => UiAction::LiveEdge,
            OverlayAction::Like => UiAction::Like,
            OverlayAction::CycleQuality => UiAction::CycleQuality,
            OverlayAction::CycleCodec => UiAction::CycleCodec,
            OverlayAction::Login => UiAction::Login,
            OverlayAction::OpenList(tab) => UiAction::OpenList(match tab {
                ListTab::Recommend => ListSource::Recommend,
                ListTab::Subs => ListSource::Subs,
                ListTab::Playlist => ListSource::Playlist,
                ListTab::History => ListSource::History,
            }),
            OverlayAction::Play { video_id } => UiAction::Play { video_id },
            OverlayAction::OpenChannel { id, name } => UiAction::OpenChannel { id, name },
            OverlayAction::OpenCardMenu(idx) => UiAction::OpenCardMenu(idx),
            OverlayAction::CloseCardMenu => UiAction::CloseCardMenu,
            OverlayAction::SaveWatchLater(video_id) => UiAction::SaveWatchLater { video_id },
            OverlayAction::NotInterested(token) | OverlayAction::NotRecommendChannel(token) => {
                UiAction::Feedback { token }
            }
            OverlayAction::CloseList => UiAction::CloseList,
            OverlayAction::ListScroll(d) => UiAction::ListMove { delta: d },
            OverlayAction::ToggleChat => UiAction::ToggleChat,
            OverlayAction::ChatScroll(d) => UiAction::ChatScroll(d),
            OverlayAction::SetChatWidth(r) => UiAction::SetChatWidth(r as f32),
            OverlayAction::ChatFontDec => UiAction::ChatFontBy(-2.0),
            OverlayAction::ChatFontInc => UiAction::ChatFontBy(2.0),
            OverlayAction::ToggleEq => UiAction::ToggleEqPanel,
            OverlayAction::SetEqVoice(db) => UiAction::SetEqVoice(db),
            OverlayAction::SetEqLowpass(hz) => UiAction::SetEqLowpass(hz),
            OverlayAction::SetEqHighpass(hz) => UiAction::SetEqHighpass(hz),
            OverlayAction::EqReset => UiAction::EqOff,
        }
    }
}

impl NativeRunning {
    /// 画質を変更し、現在の URL が YouTube なら再解決する（挙動不変。判断のクロス集約は PR B）。
    pub(super) fn set_quality(&mut self, q: Quality) {
        playback::set_quality(&mut self.playback, q);
        if resolve::is_youtube_url(self.playback.current_url()) {
            let u = self.playback.current_url().to_string();
            playback::start_resolve(&mut self.playback, u, self.account.token());
        }
    }

    /// コーデックを変更し、現在の URL が YouTube なら再解決する。
    pub(super) fn set_codec(&mut self, c: Codec) {
        playback::set_codec(&mut self.playback, c);
        if resolve::is_youtube_url(self.playback.current_url()) {
            let u = self.playback.current_url().to_string();
            playback::start_resolve(&mut self.playback, u, self.account.token());
        }
    }

    /// 現在の EQ を読み、`f` で1フィールドだけ差し替えて `set_eq` に渡す（EQ の相対/絶対
    /// 変更アクションに共通の read-modify-write パターンをまとめたもの）。
    fn update_eq(&mut self, f: impl FnOnce(&mut EqParams)) {
        let mut eq = self.playback.eq();
        f(&mut eq);
        playback::set_eq(&mut self.playback, eq);
    }

    /// チャットパネルの表示トグル（3 入力系統の共通実装。旧ドリフト: devtools/キーボード版は
    /// 固定 0.28・scroll 未リセットだったが、ユーザーが調整した幅を尊重するオーバーレイ版の
    /// 挙動に統一する — issue #11 PR B で明示された唯一の挙動変更）。
    pub(super) fn toggle_chat(&mut self) {
        self.chat_open = !self.chat_open;
        if self.chat_open {
            self.chat_scroll = 0;
        }
        let m = if self.chat_open { self.chat_width_ratio } else { 0.0 };
        self.player().set_video_margin_right(m as f64);
    }

    /// 相対シーク（秒）。dev-tools の seek_fwd/seek_back・キーボードの ←→ で使う。
    pub(super) fn seek_by(&mut self, secs: f64) {
        self.player().seek_relative(secs);
    }

    /// チャット欄の幅を相対変更する。dev-tools の chat_wider/chat_narrower で使う。
    pub(super) fn chat_width_by(&mut self, delta: f32) {
        self.chat_width_ratio = (self.chat_width_ratio + delta).clamp(0.15, 0.6);
        if self.chat_open {
            self.player().set_video_margin_right(self.chat_width_ratio as f64);
        }
    }

    /// 現在の一覧ソースを取得し直す（一覧を開くたびに 0 から組み立てる）。
    /// 手元の旧データはキャッシュではない — いつ取得したか不明な内容を見せないため
    /// 取得開始時に消える（content 側 begin_fetch）。取得中は busy を見て「取得中…」を表示。
    /// 多重リクエスト防止の busy ガードは content 側の各 start_* が持つ。
    pub(super) fn refresh_source(&mut self) {
        // トークンが失効していたら更新を先に開始する。その間の start_* は token=None で
        // 何もしないが、更新完了時の TokenRefreshed がこの refresh_source をやり直す。
        account::ensure_fresh_token(&mut self.account, &self.waker);
        match self.list_source {
            ListSource::Subs => self.start_subs(),
            ListSource::History => self.start_history(),
            ListSource::Playlist => {
                // 動画一覧を開いている間は再取得しない（一覧取得はリスト一覧へ戻す操作のため）。
                if !self.playlist.is_items_view() {
                    self.start_playlist_list();
                }
            }
            ListSource::Recommend => self.start_recommend(),
            // チャンネルビューは open_channel で取得済み。ここでは何もしない。
            ListSource::Channel => {}
        }
    }

    /// 背景スレッドからの結果を取り込み、跨ぎイベントを routing する（flows::on_logged_in）。
    pub(super) fn poll_auth(&mut self) {
        for ev in account::poll(&mut self.account) {
            match ev {
                account::AccountEvent::LoggedIn => {
                    flows::on_logged_in(&mut self.playback, &self.account);
                    // ログイン前に開かれた一覧は取得できていないので、開いたままなら取得し直す
                    // （TokenRefreshed と同じ routing。先読みはしない — 開くたび取得で十分）。
                    if self.list_open {
                        self.refresh_source();
                    }
                }
                account::AccountEvent::LoginFailed => {
                    // ログインに失敗しても、保留中の動画は匿名で解決を試みる（最善努力）。
                    if let Some(url) = playback::take_pending(&mut self.playback) {
                        playback::start_resolve(&mut self.playback, url, None);
                    }
                }
                account::AccountEvent::TokenRefreshed => {
                    // 更新待ちで保留していた再生を新しいトークンで解決する。
                    // 保留中に飛ばした履歴マークもここで送る（flows::on_logged_in と同じ理由）。
                    if let Some(url) = playback::take_pending(&mut self.playback) {
                        account::start_mark_watched_if_logged_in(self.account.token(), &url);
                        playback::start_resolve(&mut self.playback, url, self.account.token());
                    }
                    // 失効中に開かれた一覧は取得できていないので、開いたままならやり直す。
                    if self.list_open {
                        self.refresh_source();
                    }
                }
            }
        }
    }

    /// チャット更新を取り込む。NotLive を受けたらセッションを破棄する（Drop がポーラーを止める）。
    pub(super) fn poll_chat(&mut self) {
        if let Some(session) = self.chat.as_mut() {
            if !chat::poll(session) {
                self.chat = None;
            }
        }
    }

    /// 再生開始 + チャット接続（旧 Controller::load + start_chat のコンボ）。
    pub(super) fn play(&mut self, url: &str) {
        // PR4: 新 URL 再生は必ず Mpv 経路で開始する。WebView2 は SABR 詰みの救済経路で、
        // 同 URL の中で完結する（Webview→Mpv の戻し契機は「別 URL の再生開始」）。
        // 直前が Webview モードのまま残っていたら子窓 hide が偏るので、強制リセット。
        #[cfg(windows)]
        {
            self.mode = PlaybackMode::Mpv;
            self.apply_mode_visibility();
        }
        // トークンが失効していたら更新を先に開始する。flows::play は更新中（token=None かつ
        // busy）なら解決を保留し、TokenRefreshed が新トークンで解決し直す。
        account::ensure_fresh_token(&mut self.account, &self.waker);
        flows::play_with_chat(&mut self.playback, &mut self.chat, &self.account, url, &self.waker);
    }

    /// 動画を「後で見る」に保存する（ケバブメニュー）。fire-and-forget。
    pub(super) fn save_watch_later(&self, video_id: String) {
        let Some(token) = self.account.token() else { return };
        account::save_watch_later(token, video_id);
    }

    /// feedbackToken を送信する（興味なし／チャンネルをおすすめに表示しない）。fire-and-forget。
    pub(super) fn send_card_feedback(&self, token: String) {
        let Some(access_token) = self.account.token() else { return };
        account::send_card_feedback(access_token, token);
    }

    /// おすすめ（ホームフィード）を背景スレッドで取得する。要ログイン。
    pub(super) fn start_recommend(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_recommend(&mut self.recommend, &token, &self.waker);
    }

    /// 登録チャンネルタブのデータを背景スレッドで取得する。
    pub(super) fn start_subs(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_subs(&mut self.subs, &token, &self.waker);
    }

    /// 再生履歴を背景スレッドで取得する。
    pub(super) fn start_history(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_history(&mut self.history, &token, &self.waker);
    }

    /// 自分の再生リスト一覧を背景スレッドで取得する。
    pub(super) fn start_playlist_list(&mut self) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_playlist_list(&mut self.playlist, &token, &self.waker);
    }

    /// 選択した再生リストの動画一覧を背景スレッドで取得する。
    pub(super) fn start_playlist_items(&mut self, playlist_id: String, title: String) {
        let Some(token) = self.account.token() else { return };
        let token = token.to_string();
        content::start_playlist_items(&mut self.playlist, playlist_id, title, &token, &self.waker);
    }

    /// 再生リスト一覧に戻る（動画一覧を閉じる）。
    pub(super) fn playlist_back_to_lists(&mut self) {
        content::back_to_lists(&mut self.playlist);
    }

    /// ログイン（ブラウザで承認 → バックエンドでトークン取得 → チャンネル名取得）を背景で開始。
    pub(super) fn start_login(&mut self) {
        account::start_login(&mut self.account, &self.waker);
    }

    /// 現在の動画に高評価を付ける（必要ならトークンを更新してから）を背景で開始。
    pub(super) fn start_like(&mut self, video_id: String) {
        account::start_like(&mut self.account, video_id, &self.waker);
    }

    /// ライブチャットのポーリングを停止する。
    pub(super) fn stop_chat(&mut self) {
        self.chat = None;
    }

    /// チャンネル名からそのチャンネルの動画一覧を背景取得する（名前→channelId→browse）。
    pub(super) fn open_channel(&mut self, name: String) {
        content::open_channel(&mut self.channel_view, name, &self.waker);
    }

    /// 実 channelId(UC...) からそのチャンネルの動画一覧を背景取得する。
    pub(super) fn open_channel_by_id(&mut self, id: String, title: String) {
        content::open_channel_by_id(&mut self.channel_view, id, title, &self.waker);
    }

    /// dev-tools のアクション名を `UiAction` に変換して `apply_action` へ渡す
    /// （キーボード/オーバーレイの全操作を網羅）。既知なら true。
    pub(super) fn devtools_action(&mut self, name: &str) -> bool {
        let action = match name {
            "play_pause" => UiAction::TogglePause,
            "seek_fwd" => UiAction::SeekBy(5.0),
            "seek_back" => UiAction::SeekBy(-5.0),
            "live_edge" => UiAction::LiveEdge,
            "vol_up" => UiAction::VolumeBy(5.0),
            "vol_down" => UiAction::VolumeBy(-5.0),
            "mute" => UiAction::ToggleMute,
            "eq_voice_up" => UiAction::EqVoiceBy(1.0),
            "eq_voice_down" => UiAction::EqVoiceBy(-1.0),
            "eq_lowpass_up" => UiAction::EqLowpassStep(1),
            "eq_lowpass_down" => UiAction::EqLowpassStep(-1),
            "eq_highpass_up" => UiAction::EqHighpassStep(1),
            "eq_highpass_down" => UiAction::EqHighpassStep(-1),
            "eq_off" => UiAction::EqOff,
            "eq_toggle" => UiAction::ToggleEqPanel,
            "quality_next" => UiAction::CycleQuality,
            "codec_next" => UiAction::CycleCodec,
            "toggle_chat" => UiAction::ToggleChat,
            "chat_font_inc" => UiAction::ChatFontBy(2.0),
            "chat_font_dec" => UiAction::ChatFontBy(-2.0),
            "chat_scroll_up" => UiAction::ChatScroll(3),
            "chat_scroll_down" => UiAction::ChatScroll(-3),
            "chat_wider" => UiAction::ChatWidthBy(0.04),
            "chat_narrower" => UiAction::ChatWidthBy(-0.04),
            "login" => UiAction::Login,
            "like" => UiAction::Like,
            "play_url" => UiAction::PlayUrl(self.url_input.trim().to_string()),
            "toggle_list" => UiAction::ToggleList,
            "close_overlay" => UiAction::CloseList,
            "open_recommend" => UiAction::OpenList(ListSource::Recommend),
            "open_subs" => UiAction::OpenList(ListSource::Subs),
            "open_playlist" => UiAction::OpenList(ListSource::Playlist),
            "open_history" => UiAction::OpenList(ListSource::History),
            "list_up" => UiAction::ListMove { delta: -1 },
            "list_down" => UiAction::ListMove { delta: 1 },
            "list_select" => UiAction::ListSelect,
            "list_back" => UiAction::ListBack,
            _ => return false,
        };
        self.apply_action(action)
    }

    /// 一覧の行 index から ID を引いて再生する（devtools/キーボードが使う index ベースの入口）。
    /// 描画順の座席番号(index)をここで一度だけ ID に変換し、以降は [`Self::play_by_id`] という
    /// ID ベースの経路に合流させる（オーバーレイの直接クリックと処理を共有する）。
    pub(super) fn play_list_index(&mut self, idx: usize) {
        if self.list_source == ListSource::Playlist && !self.playlist.is_items_view() {
            if let Some(pl) = self.playlist.lists().get(idx) {
                self.play_by_id(pl.playlist_id.clone());
            }
            return;
        }
        let rows = self.list_rows().1;
        if let Some(card) = rows.get(idx) {
            self.play_by_id(card.id.clone());
        }
    }

    /// 一覧の行の実 ID を再生する（再生リスト 1 階層目なら ID は playlist_id で、中身を開く）。
    /// オーバーレイの直接クリック（`OverlayAction::Play`）と `play_list_index` の共通処理。
    pub(super) fn play_by_id(&mut self, video_id: String) {
        self.card_menu_open = None;
        if self.list_source == ListSource::Playlist && !self.playlist.is_items_view() {
            // 再生リスト一覧で選択 → その中身を開く（2 階層目へ）。ここでの ID は playlist_id。
            if let Some(pl) = self.playlist.lists().iter().find(|p| p.playlist_id == video_id) {
                let title = pl.title.clone();
                self.list_sel = 0;
                self.start_playlist_items(video_id.clone(), title);
            }
            return;
        }
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        self.list_open = false;
        self.url_input = url.clone();
        self.play(&url);
    }

    /// 3 入力系統（オーバーレイ/dev-tools/キーボード）の合流点。全員がここへ `UiAction` を
    /// 組み立てて渡すだけにする（Issue #11 PR B）。
    /// 戻り値 = 「ユーザー操作があったか」（呼び出し側 shell が last_activity 更新に使う）。
    pub(super) fn apply_action(&mut self, a: UiAction) -> bool {
        match a {
            UiAction::TogglePause => {
                let p = self.player();
                p.set_paused(!p.paused());
            }
            UiAction::SeekTo(frac) => {
                let p = self.player();
                let dur = p.duration();
                if p.seekable() && dur > 0.0 {
                    p.set_time_pos(frac * dur);
                }
            }
            UiAction::SeekBy(secs) => self.seek_by(secs),
            UiAction::SetVolume(v) => self.player().set_volume(v.clamp(0.0, 130.0)),
            UiAction::VolumeBy(d) => {
                let p = self.player();
                p.set_volume((p.volume() + d).clamp(0.0, 130.0));
            }
            UiAction::ToggleMute => {
                let p = self.player();
                p.set_muted(!p.muted());
            }
            UiAction::LiveEdge => self.player().seek_to_live(),
            UiAction::Like => {
                if let Some(vid) = auth::extract_video_id(self.current_url()) {
                    self.start_like(vid);
                }
            }
            UiAction::CycleQuality => {
                let all = Quality::ALL;
                let i = all.iter().position(|q| *q == self.quality()).unwrap_or(0);
                self.set_quality(all[(i + 1) % all.len()]);
            }
            UiAction::CycleCodec => {
                let all = Codec::ALL;
                let i = all.iter().position(|c| *c == self.codec()).unwrap_or(0);
                self.set_codec(all[(i + 1) % all.len()]);
            }
            UiAction::Login => {
                if !self.account.is_busy() {
                    self.start_login();
                }
            }
            UiAction::PlayUrl(url) => {
                if !url.is_empty() {
                    self.play(&url);
                }
            }
            UiAction::OpenList(src) => {
                self.list_source = src;
                self.list_open = true;
                self.list_sel = 0;
                self.card_menu_open = None;
                self.refresh_source();
            }
            UiAction::Play { video_id } => self.play_by_id(video_id),
            UiAction::OpenChannel { id, name } => {
                // 実 channelId があればそれを使い（確実）、無ければ名前検索にフォールバックする。
                if let Some(id) = id {
                    self.open_channel_by_id(id, name);
                    self.list_source = ListSource::Channel;
                    self.list_sel = 0;
                } else if !name.is_empty() {
                    self.open_channel(name);
                    self.list_source = ListSource::Channel;
                    self.list_sel = 0;
                }
                self.card_menu_open = None;
            }
            UiAction::OpenCardMenu(idx) => {
                self.card_menu_open = Some(idx);
            }
            UiAction::CloseCardMenu => {
                self.card_menu_open = None;
            }
            UiAction::SaveWatchLater { video_id } => {
                self.save_watch_later(video_id);
                self.card_menu_open = None;
            }
            UiAction::Feedback { token } => {
                self.send_card_feedback(token);
                self.card_menu_open = None;
            }
            UiAction::CloseList => {
                self.list_open = false;
                self.card_menu_open = None;
            }
            UiAction::ToggleList => {
                self.list_open = !self.list_open;
                self.card_menu_open = None;
                if self.list_open {
                    self.list_sel = 0;
                    self.refresh_source();
                }
            }
            UiAction::ListMove { delta } => {
                let n = self.list_rows().1.len();
                if n > 0 {
                    let sel = (self.list_sel as i32 + delta).clamp(0, n as i32 - 1);
                    self.list_sel = sel as usize;
                }
            }
            UiAction::ListSelect => {
                self.card_menu_open = None;
                self.play_list_index(self.list_sel);
            }
            UiAction::ListBack => {
                if self.list_source == ListSource::Channel {
                    // チャンネルビューから おすすめ へ戻る。
                    self.list_source = ListSource::Recommend;
                    self.list_sel = 0;
                } else if self.list_source == ListSource::Playlist && self.playlist.is_items_view() {
                    self.playlist_back_to_lists();
                    self.list_sel = 0;
                }
            }
            UiAction::ToggleChat => self.toggle_chat(),
            UiAction::ChatScroll(d) => {
                let max = self.chat.as_ref().map_or(0, |c| c.messages().len()).saturating_sub(1);
                self.chat_scroll = ((self.chat_scroll as i32 + d).max(0) as usize).min(max);
            }
            UiAction::ChatFontBy(d) => {
                self.chat_font_px = (self.chat_font_px + d).clamp(10.0, 28.0);
            }
            UiAction::SetChatWidth(r) => {
                self.chat_width_ratio = r.clamp(0.15, 0.6);
                if self.chat_open {
                    self.player().set_video_margin_right(self.chat_width_ratio as f64);
                }
            }
            UiAction::ChatWidthBy(d) => self.chat_width_by(d),
            UiAction::EqVoiceBy(d) => self.update_eq(|eq| eq.voice_gain_db += d),
            UiAction::EqLowpassStep(dir) => {
                self.update_eq(|eq| eq.lowpass_hz = EqParams::lowpass_step(eq.lowpass_hz, dir))
            }
            UiAction::EqHighpassStep(dir) => {
                self.update_eq(|eq| eq.highpass_hz = EqParams::highpass_step(eq.highpass_hz, dir))
            }
            UiAction::EqOff => {
                playback::set_eq(&mut self.playback, EqParams::default());
            }
            UiAction::SetEqVoice(db) => self.update_eq(|eq| eq.voice_gain_db = db),
            UiAction::SetEqLowpass(hz) => self.update_eq(|eq| eq.lowpass_hz = hz),
            UiAction::SetEqHighpass(hz) => self.update_eq(|eq| eq.highpass_hz = hz),
            UiAction::ToggleEqPanel => {
                self.eq_open = !self.eq_open;
            }
        }
        true
    }

    /// キーボード入力（`WindowEvent::KeyboardInput`）の処理。地雷ではない「普通のコード」
    /// なので shell から呼ばれるだけの薄いメソッドにしてある。
    /// 戻り値 = 「ユーザー操作があったか」。last_activity の更新は呼び出し側（shell）だけが行う
    /// （Issue #11 PR U §9.2: 跨ぎ状態には触らず、戻り値で伝える）。
    pub(super) fn handle_keyboard(&mut self, event: KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        // Ctrl+修飾キー: L=ログイン, G=高評価, Q=画質切替, C=コーデック切替。
        // 挙動不変のため、旧実装がここで last_activity を更新していなかった点（他の
        // キー入力と異なり早期 return していた）もそのまま踏襲する（Ctrl+V のみ例外）。
        if self.ctrl {
            if let Key::Character(c) = &event.logical_key {
                let action = match c.as_str().to_ascii_lowercase().as_str() {
                    "l" => Some(UiAction::Login),
                    "g" => Some(UiAction::Like),
                    "t" => Some(UiAction::ToggleChat),
                    "q" => Some(UiAction::CycleQuality),
                    "c" => Some(UiAction::CycleCodec),
                    // Ctrl + "-" / "+"（"=" も可）: コメント文字サイズ増減。
                    "-" => Some(UiAction::ChatFontBy(-2.0)),
                    "+" | "=" => Some(UiAction::ChatFontBy(2.0)),
                    _ => None,
                };
                if let Some(a) = action {
                    self.apply_action(a);
                    return false;
                }
            }
        }
        // Ctrl+V: クリップボードのテキストを URL 欄へ貼り付け（テキスト編集そのものなので
        // UiAction 化しない）。
        if self.ctrl {
            if let Key::Character(c) = &event.logical_key {
                if c.eq_ignore_ascii_case("v") {
                    #[cfg(windows)]
                    if let Some(t) = crate::dcomp_overlay::clipboard_text() {
                        for ch in t.chars() {
                            if !ch.is_control() {
                                self.url_input.push(ch);
                            }
                        }
                    }
                    return true;
                }
            }
        }
        // Tab: 一覧を開閉。
        if let Key::Named(NamedKey::Tab) = event.logical_key {
            self.apply_action(UiAction::ToggleList);
            return true;
        }
        // 一覧表示中はキーをナビゲーション／ソース切替に使う。
        if self.list_open {
            // グリッドの 1 行移動量＝現在の列数（未描画時は 1）。
            #[cfg(windows)]
            let cols = self
                .dcomp_overlay
                .as_ref()
                .map(|o| o.grid_cols())
                .unwrap_or(1)
                .max(1) as i32;
            #[cfg(not(windows))]
            let cols = 1i32;
            match &event.logical_key {
                Key::Named(NamedKey::ArrowUp) => {
                    self.apply_action(UiAction::ListMove { delta: -cols });
                }
                Key::Named(NamedKey::ArrowDown) => {
                    self.apply_action(UiAction::ListMove { delta: cols });
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.apply_action(UiAction::ListMove { delta: -1 });
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.apply_action(UiAction::ListMove { delta: 1 });
                }
                Key::Named(NamedKey::Enter) => {
                    // devtools の list_select と同じ経路（旧: ここだけ play_list_index を
                    // 呼ばずインライン再実装していたドリフトを解消。issue #11 PR B）。
                    self.apply_action(UiAction::ListSelect);
                }
                Key::Named(NamedKey::Backspace) => {
                    self.apply_action(UiAction::ListBack);
                }
                Key::Named(NamedKey::Escape) => {
                    self.apply_action(UiAction::CloseList);
                }
                Key::Character(c) => {
                    self.card_menu_open = None;
                    let src = match c.as_str() {
                        "1" => Some(ListSource::Subs),
                        "2" => Some(ListSource::Recommend),
                        "3" => Some(ListSource::History),
                        "4" => Some(ListSource::Playlist),
                        _ => None,
                    };
                    if let Some(src) = src {
                        self.apply_action(UiAction::OpenList(src));
                    }
                }
                _ => {}
            }
            return true;
        }
        match event.logical_key {
            // Space は URL に現れないため再生/一時停止に温存。
            Key::Named(NamedKey::Space) => {
                self.apply_action(UiAction::TogglePause);
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.apply_action(UiAction::SeekBy(5.0));
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.apply_action(UiAction::SeekBy(-5.0));
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.apply_action(UiAction::VolumeBy(5.0));
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.apply_action(UiAction::VolumeBy(-5.0));
            }
            // --- URL 入力欄の編集（テキスト編集そのものなので UiAction 化しない）---
            Key::Named(NamedKey::Backspace) => {
                self.url_input.pop();
            }
            Key::Named(NamedKey::Escape) => self.url_input.clear(),
            Key::Named(NamedKey::Enter) => {
                self.apply_action(UiAction::PlayUrl(self.url_input.trim().to_string()));
            }
            // 印字可能文字は URL 欄へ追記（IME 不要。URL は英数字記号のみ）。
            _ => {
                if let Some(t) = &event.text {
                    for ch in t.chars() {
                        if !ch.is_control() {
                            self.url_input.push(ch);
                        }
                    }
                }
            }
        }
        // キー操作も活動として扱う（戻り値経由で shell に伝える）。
        true
    }
}
