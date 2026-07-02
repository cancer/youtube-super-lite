# OAuthログインのセットアップ手順

対象読者: このアプリを自分でビルド/配布し、ログイン機能（高評価・登録チャンネル・履歴・再生リスト等）を
使えるようにしたい人。機能そのものの説明は [../features/login-and-rating.md](../features/login-and-rating.md) を参照。

client_secret を含む認証設計の背景は [../design/auth-backend.md](../design/auth-backend.md) を参照。

## 1. Google 側

1. [Google Cloud Console](https://console.cloud.google.com/) でプロジェクト作成
2. **YouTube Data API v3** を有効化
3. **OAuth 同意画面**（User Type: 外部）。テストユーザーに自分のアカウントを追加。スコープ
   `.../auth/youtube.force-ssl`
4. **OAuth クライアント ID** を作成。種類は **デスクトップ アプリ**（`127.0.0.1` への戻りが許可される）。
   client_id と client_secret を控える

## 2. Worker のデプロイ（[auth-worker/](../../auth-worker/)）

`wrangler`（`npm i -g wrangler`）と Rust + `wasm32-unknown-unknown` ターゲットが必要。

```bash
cd auth-worker
# wrangler.jsonc の vars GAUTH_CLIENT_ID を自分の client_id に書き換える
wrangler secret put GAUTH_CLIENT_SECRET    # client_secret を貼り付け（配布物には入らない）
wrangler deploy                            # → https://<worker-name>.<account>.workers.dev
```

## 3. アプリ側

既定では本番 Worker（`https://youtube-super-lite-backend.cancer6.workers.dev`）に接続する。ローカルの
`wrangler dev` 等で別の Worker を使う場合のみ `--debug-backend <URL>` で上書きする。

セットアップが完了すれば、アプリ内でのログイン操作（ログインボタン or `Ctrl+L`）でこの Worker 経由の
OAuthフローが動く。動作の詳細は [../features/login-and-rating.md](../features/login-and-rating.md)。
