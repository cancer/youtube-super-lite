# ログイン・高評価

対象読者: ログインすると何ができるようになるか、どう使うかを確認したい人。
セットアップ（Google Cloud Console / Worker デプロイ）の手順は [../setup/oauth-setup.md](../setup/oauth-setup.md) を参照。

## Why

高評価・登録チャンネル・履歴・再生リストの閲覧、視聴履歴のYouTube本家への反映はすべて、その人個人の
YouTubeアカウントに紐づく操作であり、ログインなしには成立しない。一方で、配布アプリに OAuth の
client_secret をそのまま埋め込むのは安全ではない。そのためこのアプリは「ブラウザでの同意」だけを
自前で行い、secret が必要なトークン交換は別途デプロイしたバックエンドに任せる設計にしている
（設計の詳細: [../design/auth-backend.md](../design/auth-backend.md)）。

## ログインで使えるようになる機能

| 機能 | ドキュメント |
|------|------|
| 高評価 | 本ページ |
| 登録チャンネル新着・履歴の一覧 | [browse-lists.md](browse-lists.md) |
| 再生リストの一覧・中身 | [playlists.md](playlists.md) |
| 視聴履歴のYouTube本家への反映 | [watch-history-tracking.md](watch-history-tracking.md) |

## 使い方

未ログイン時は上部ナビゲーション帯の右寄せに黄色いログインボタンが表示される。これをクリックする
（キーボードなら `Ctrl+L`）と既定ブラウザで Google の同意画面が開き、同意するとアプリに自動で
戻ってログイン状態になる（[controller-ui.md](controller-ui.md)）。

ログイン中は下部コントローラの 👍 ボタンをクリック（キーボードなら `Ctrl+G`）すると、現在再生中の
動画に高評価を付けられる（`videos.rate`）。現在の評価状態の表示・トグル（`videos.getRating` による
「すでに高評価済みか」の表示）は未実装。

## ログイン状態の保持

リフレッシュトークンは `%APPDATA%\YouTubeSuperLite\auth.json` に保存され、次回起動時は自動ログインを
試みる（再度ブラウザでの同意操作は不要）。

起動中もログインセッションは自動で継続する。アクセストークンの寿命は約 1 時間で、失効を検知すると
リフレッシュトークンで背景更新する（ステータス表示は「セッション更新中…」）。更新中に開始した
再生・一覧取得は、更新完了後に自動でやり直されるため、ユーザー操作は不要。更新に失敗した場合は
30 秒間隔で再試行する。

## 関連

- [../setup/oauth-setup.md](../setup/oauth-setup.md) — Google Cloud Console / Worker デプロイ手順
- [../design/auth-backend.md](../design/auth-backend.md) — なぜ client_secret を配布アプリに持たせないか
