# 認証バックエンドの設計（OAuth + Cloudflare Worker）

対象読者: ログイン周りの設計判断を確認したい人、認証バックエンドをデプロイ/変更したい人。

## 設計方針: client_secret を配布アプリに持たせない

OAuth の同意画面（ブラウザ操作）だけをアプリが担当し、`client_secret` が必要なトークン交換（authorization
code → access/refresh token）は別途デプロイした **Cloudflare Worker**（[auth-worker/](../../auth-worker/)）
が代行する。アプリが知っているのは「Worker の URL」だけで、client_secret はアプリのバイナリにも設定ファイルにも
含まれない。

```
[アプリ] ─ブラウザで同意→ループバックでcode ─▶ [Worker] ─(id+secret付与)→ Google
        ◀──────── access / refresh token ───────
        ── Data API / InnerTube を直接 ──────▶ YouTube
```

## フロー

1. アプリがループバック HTTP サーバ（`127.0.0.1`）を一時的に立て、既定ブラウザで Google の OAuth 同意画面を開く
   （`client_id` は埋め込み済み、種類は「デスクトップ アプリ」でループバックへのリダイレクトが許可される）
2. 同意後、Google がループバックへ `authorization code` 付きでリダイレクトする
3. アプリはその code を Worker に渡し、Worker が `client_secret` を付与して Google のトークンエンドポイントへ
   交換リクエストを送る
4. Worker は access token / refresh token をアプリへ返す。以降アプリはこれらのトークンで Data API v3 /
   InnerTube（TVHTML5 + Bearer）を直接呼ぶ

## トークンの保存と更新

- refresh token は `%APPDATA%\YouTubeSuperLite\auth.json` に保存し、次回起動時は自動でサイレントログインを試みる
- access token の有効期限はローカルで管理し（期限の 60 秒前を expiry とみなす）、API 呼び出し前に
  期限切れなら Worker 経由で自動的にリフレッシュする（呼び出し側は意識しなくてよい）

## デバッグ用の切り替え

既定では本番 Worker（`https://youtube-super-lite-backend.cancer6.workers.dev`）に接続する。ローカルで
`wrangler dev` した Worker を使いたい場合のみ、CLI の `--debug-backend <URL>` で上書きする。

## 関連

- [../features/login-and-rating.md](../features/login-and-rating.md) — ユーザー向けのセットアップ手順・機能
- [auth-worker/](../../auth-worker/) — Worker の実装
