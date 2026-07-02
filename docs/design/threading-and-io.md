# スレッドモデルと I/O の設計

対象読者: 新しい非同期処理（API呼び出し・外部プロセス呼び出しなど）を追加する人。

## 基本方針

このプロジェクトは async ランタイム（tokio 等）を使わず、**OS スレッド + `std::sync::mpsc` + winit の
`EventLoopProxy`** で非同期処理を組んでいる。

パターンは共通で、以下の形を取る:

1. `Controller`（またはその呼び出し元）が `std::thread::spawn` でバックグラウンドスレッドを起動する
2. スレッドは `reqwest::blocking::Client` などブロッキングI/Oで処理を行う
3. 結果を機能ごとの `mpsc::Sender` で送る（例: `ChatUpdate` / `RecommendUpdate` / `SubUpdate` /
   `HistoryUpdate` / `PlaylistUpdate` / `ResolveUpdate` / `AuthMsg`）
4. 送信後、winit の `EventLoopProxy::send_event(UserEvent::Background)` でメインループを起こす
5. メインループ側（`native_app` / `Controller`）は起床のたびに各チャンネルを `try_recv()` でポーリングし、
   結果を状態へ反映する

この設計により、メインスレッド（winit のイベントループ）は一切ブロッキング I/O を行わず、CPU バウンドな
イベント処理と描画要求のディスパッチに専念できる。

## 常駐ワーカー vs 都度スレッド

大半のバックグラウンド処理（チャットポーリング・一覧取得・認証）は「都度スレッド」（リクエストごとに
`thread::spawn`）で十分だが、**URL解決だけは例外的に常駐ワーカースレッド**を使う（[url-resolution.md](url-resolution.md)）。
理由は、訪問者セッション Cookie や nsig の解決結果（player バージョンごとにキャッシュ）を都度スレッドでは
使い回せないため。常駐ワーカーは `ResolveRequest` を受け取るチャンネルを持ち続け、アプリ起動時に一度だけ立ち上がる。

## インタラプト可能な待機

チャットポーリング（[../features/chat.md](../features/chat.md)）のように「一定間隔でポーリングしつつ、
チャットを閉じたら即座に止めたい」という要求があるループは、単純な `thread::sleep` ではなく
インタラプト可能なスリープ（停止シグナルを見ながら短い間隔で待つ）を使い、UI操作への追従性を確保している。

## デバッグ用 HTTP サーバとの接続

`devtools`（[../features/devtools.md](../features/devtools.md)）のローカル HTTP サーバも同じパターンに従う。
`tiny_http` によるリクエスト受信は専用スレッドで行い、コマンドは mpsc チャンネル経由でメインスレッドに転送、
`EventLoopProxy` で処理を起こしてから応答を返す（1 リクエストあたり 5 秒タイムアウト）。

## 関連

- [architecture-overview.md](architecture-overview.md) — 全体設計の中でのこのパターンの位置付け
