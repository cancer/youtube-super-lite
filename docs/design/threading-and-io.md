# スレッドモデルと I/O の設計

対象読者: 新しい非同期処理（API呼び出し・外部プロセス呼び出しなど）を追加する人。

## 基本方針

このプロジェクトは async ランタイム（tokio 等）を使わず、**OS スレッド + `std::sync::mpsc` + `Waker`**
で非同期処理を組んでいる。`Waker`（`crates/ysl-core::Waker` = `Arc<dyn Fn() + Send + Sync>`）は、
lib が winit に依存できないことの帰結で、bin 側が winit の `EventLoopProxy` を包んで各ドメインの
`start_*` に注入する抽象。

パターンは共通で、以下の形を取る:

1. 各ドメインの `start_*` system 関数（例: `content::start_recommend`, `account::start_login`）が
   `std::thread::spawn` でバックグラウンドスレッドを起動する
2. スレッドは `reqwest::blocking::Client` などブロッキングI/Oで処理を行う
3. 結果を機能ごとの `mpsc::Sender` で送る（例: `chat::ChatUpdate` / `content::FeedUpdate<T>`
   （旧 `RecommendUpdate`/`SubUpdate`/`HistoryUpdate` を統一）/ `playlist::PlaylistUpdate` /
   `resolve::ResolveUpdate` / `account::AuthMsg`）
4. 送信後、注入された `Waker` を呼んでメインループを起こす（bin 側で winit の
   `EventLoopProxy::send_event(UserEvent::Background)` に変換される）
5. メインループ側（`ui::shell::NativeRunning`）は起床のたびに各ドメインの `poll_*` system 関数を呼び、
   結果を状態へ反映する

この設計により、メインスレッド（winit のイベントループ）は一切ブロッキング I/O を行わず、CPU バウンドな
イベント処理と描画要求のディスパッチに専念できる。

## 常駐ワーカー vs 都度スレッド

大半のバックグラウンド処理（チャットポーリング・一覧取得・認証）は「都度スレッド」（リクエストごとに
`thread::spawn`）で十分だが、**URL解決だけは例外的に常駐ワーカースレッド**を使う（[url-resolution.md](url-resolution.md)）。
理由は、訪問者セッション Cookie（`VISITOR_INFO1_LIVE`）を都度スレッドでは使い回せないため。加えて、
JSエンジン（boa の `Context`）は `!Send` で複数スレッド間を渡せない制約もあり、これも1本のワーカーに
留める設計を後押ししている（ただしこのJSエンジン自体は現状 [url-resolution.md](url-resolution.md) に
書いた通り実際の解決経路では使われていない）。常駐ワーカーは `ResolveRequest` を受け取るチャンネルを
持ち続け、アプリ起動時に一度だけ立ち上がる。

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
