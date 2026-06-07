# タスク: 動画の概要/説明文の表示

> 背景: ネイティブ版 UI parity（[native-ui-parity-tasks.md](native-ui-parity-tasks.md)）完了時の
> 「未確定」項目より。egui 版にも無かった（タイトルのみ表示）が、ユーザー要望により別タスク化。
> 現状ネイティブ版の上部バーは動画タイトル（`media_title`）のみ表示している。

## 要件（案）
- [ ] 動画の説明文（description）の取得元を決める
  - mpv の `media-title` 同様のメタデータには description は無い。yt-dlp の `--print description` か
    `-J`(JSON) で取得するのが素直（resolve.rs の解決フローに相乗りできるか検討）。
  - もしくは Data API v3 `videos.list?part=snippet` で description を取得（要 video_id・APIキー/OAuth）。
- [ ] Controller に description フィールドを追加し、再生開始時に背景取得 → poll で反映
- [ ] native_overlay 側に説明文の表示 UI を追加
  - 表示場所: 一覧（Tab）とは別の専用パネル？ それともコントローラ上部のトグル表示？
  - 長文・改行・スクロールの扱い（DirectWrite のレイアウト）。
- [ ] 開閉トグル（キーボード＋オーバーレイのボタン）。チャット（💬）と同様の右/下パネル方式が候補。

## 不明点（要確認）
- 取得元: yt-dlp（追加の解決コスト）か Data API（認証要・割当消費）か。
- 表示形式: 常時表示 / トグル / 一覧内 のどれにするか。
- メンバー限定や年齢制限動画での description 取得可否。
