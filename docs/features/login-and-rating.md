# ログイン・高評価

対象読者: 初めてこのアプリのログイン機能をセットアップする人。

## この機能で使えるようになるもの

高評価・登録チャンネル新着・履歴・再生リストは、いずれもログイン（OAuth）が必要。

## セットアップ手順

**client_secret は配布アプリには持たせない設計。** 認証の同意画面のみブラウザに委譲し、secret が必要な
トークン交換は別途デプロイした Cloudflare Worker が代行する（設計の詳細: [design/auth-backend.md](../design/auth-backend.md)）。

### 1. Google 側

1. [Google Cloud Console](https://console.cloud.google.com/) でプロジェクト作成
2. **YouTube Data API v3** を有効化
3. **OAuth 同意画面**（User Type: 外部）。テストユーザーに自分のアカウントを追加。スコープ
   `.../auth/youtube.force-ssl`
4. **OAuth クライアント ID** を作成。種類は **デスクトップ アプリ**（`127.0.0.1` への戻りが許可される）。
   client_id と client_secret を控える

### 2. Worker のデプロイ（[auth-worker/](../../auth-worker/)）

`wrangler`（`npm i -g wrangler`）と Rust + `wasm32-unknown-unknown` ターゲットが必要。

```bash
cd auth-worker
# wrangler.jsonc の vars GAUTH_CLIENT_ID を自分の client_id に書き換える
wrangler secret put GAUTH_CLIENT_SECRET    # client_secret を貼り付け（配布物には入らない）
wrangler deploy                            # → https://<worker-name>.<account>.workers.dev
```

### 3. アプリ側

既定では本番 Worker（`https://youtube-super-lite-backend.cancer6.workers.dev`）に接続する。ローカルの
`wrangler dev` 等で別の Worker を使う場合のみ `--debug-backend <URL>` で上書きする。アプリ内では
**Ctrl+L** でログインを開始する。

## トークンの保存

リフレッシュトークンは `%APPDATA%\YouTubeSuperLite\auth.json` に保存され、次回起動時は自動ログインを試みる。

## 高評価

ログイン中に **Ctrl+G** で現在の動画に高評価を付ける（`videos.rate`）。現在の評価状態の表示・トグル
（`videos.getRating` による「すでに高評価済みか」の表示）は未実装。

## 関連

- [design/auth-backend.md](../design/auth-backend.md) — OAuthフローとWorkerの設計
- [browse-lists.md](browse-lists.md) / [playlists.md](playlists.md) — ログインが必要な一覧機能
- [watch-history-tracking.md](watch-history-tracking.md) — ログイン時のみ有効な視聴履歴記録
