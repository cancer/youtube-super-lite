# Issue #16 PoC 作業計画書 — WebView2 + YouTube IFrame embed ライブ再生検証

対象: [Issue #16](https://github.com/cancer/youtube-super-lite/issues/16) の「次アクション: PoC」を実装する人。
このPoCの目的は **go/no-go の確定** であり、本実装ではない。ここに書かれた決定はすべて起票者との
一問一答で確定済みなので、実装中に再検討する必要はない。

## 0. これは何の検証か

ログイン済みライブが SABR 化で再生不可という問題に対し、issue #16 は「公式 YouTube IFrame プレーヤーを
WebView2 で埋め込む」ハイブリッド fallback を採用方針として決定済み。ただし本実装に進む前に、
**WebView2 に YouTube IFrame embed をロードして実際に映像が出るか** を最小コストで確認する必要がある。

- **成功条件**: AJE / NASA / LofiGirl のライブで映像が実際に再生される（1本でも複数本でも、ロード自体が
  通ることが確認できれば白）
- **このPoCで確定させたいこと**: embed の再生可否そのもの。経路切替・UI統合・ログイン対応は次工程

## 1. 決定事項（確定済み・再検討不要）

| 論点 | 決定 |
|---|---|
| WebView2統合方式 | `webview2-com`（Microsoft公式のCOMバインディング）を使う。`wry`等の高レベルラッパーは使わない |
| ライブ指定方法 | `videoId` の直接指定ではなく、チャンネルIDで `https://www.youtube.com/embed/live_stream?channel=<channelId>` を使う（現在のライブに自動解決させる。videoId のハードコードによる陳腐化を避ける） |
| 判定方法 | PoCアプリ自身が起動中に自動で数回（例: 数秒おき）ウィンドウをBitBltキャプチャし、PNGとしてローカルファイル保存する。HTTPサーバは持たない。保存されたPNGを人が目視して白黒判定する |
| スクリーンショット実装 | 既存 `src/devtools.rs` のBitBltロジックは流用・共有化しない。PoC専用に最小限のキャプチャを新規に独立実装する（重複コード許容。PoCは使い捨て前提のため） |
| コード配置 | 本体パッケージ（root `Cargo.toml`）とは完全に別の独立パッケージ・別フォルダに切り出す。`resolver-sidecar` と同様の構成（独自の `Cargo.toml` を持ち、root の `[workspace] members` に追加する。本体バイナリ(`youtube-super-lite.exe`)の依存グラフには一切混ざらない） |
| 依存追加の可否 | `webview2-com` および `windows` crateのWebView2/COM関連featureは、上記の独立パッケージ側にのみ追加してよい。本体 `Cargo.toml` は変更しない |
| 実行単位 | 1回の起動で1チャンネルのみ検証する。チャンネルIDはコマンドライン引数で指定する（3チャンネルを1プロセス内で順に切り替える実装はしない） |
| ライフサイクル | 起動 → WebView2生成・embed URLロード → ウィンドウ表示 → 数秒おきに自動スクリーンショット保存を数回 → 一定時間（例: 30秒程度）後に自動終了。ユーザーによる手動操作は不要 |
| ログイン/セッション | 対象3チャンネルは公開ライブ・埋め込み許可想定のためスコープ外。WebView2は使い捨ての一時ユーザーデータフォルダでよい（永続プロファイル・cookie引き継ぎの実装はしない） |

## 2. 検証対象チャンネル（要・実施時再確認）

| チャンネル | チャンネルID | embed URL |
|---|---|---|
| Al Jazeera English | `UCNye-wNBqNL5ZzHSJj3l8Bg` | `https://www.youtube.com/embed/live_stream?channel=UCNye-wNBqNL5ZzHSJj3l8Bg` |
| NASA | `UCLA_DiR1FfKNvjuUpBHmylQ` | `https://www.youtube.com/embed/live_stream?channel=UCLA_DiR1FfKNvjuUpBHmylQ` |
| Lofi Girl | `UCSJ4gkVC6NrvII8umztf0Ow` | `https://www.youtube.com/embed/live_stream?channel=UCSJ4gkVC6NrvII8umztf0Ow` |

上記チャンネルIDは実装時点の外部調査によるもの。**実装直前に `https://www.youtube.com/channel/<ID>/live`
が実際に現在ライブを配信中か目視確認してから使うこと**（チャンネルの改名・配信停止は起票者の与り知らぬ
外部要因のため、このPoCの判定結果を左右しないよう事前確認する）。

## 3. 実装ステップ

1. 独立パッケージを新規作成（例: `poc-webview2-iframe/`）し、root `Cargo.toml` の `[workspace] members` に追加する
2. `webview2-com` + `windows`（COM/WebView2まわりのfeature）を依存に追加する
3. コマンドライン引数でチャンネルIDを受け取り、embed URL を組み立てる
4. winitまたは生Win32でウィンドウを1つ作成し、`webview2-com` でWebView2コントローラを生成、embed URLをロードする
   （ユーザーデータフォルダは一時ディレクトリを都度生成）
5. 数秒おき・数回、対象ウィンドウのクライアント領域をBitBltでキャプチャし、連番PNGとしてローカルに保存する
6. 一定時間経過後、プロセスを自動終了する
7. AJE / NASA / LofiGirl のそれぞれについて実行し、保存されたPNG群を目視して「映像が出ているか」を判定する

## 4. 完了条件・報告

- 3チャンネルそれぞれについて「白（映像が出た）」か「黒（出なかった／エラー）」かを判定する
- 判定結果と使用したPNG（またはその要約）を issue #16 にコメントとして追記する（過去のP0/P0.5/案2 PoCの
  コメントと同じ形式に揃える）
- 白の場合 → 本実装（経路切替配線・UI統合・「YouTubeで開く」fallback）へ進む価値が確定
- 黒の場合 → 埋め込み可否・原因（埋め込み禁止／WebView2側のエラー等）を切り分けて次アクションを再検討

## 4.5 実装時の修正（計画からの逸脱）

計画時点では `webview.Navigate(embed_url)` でWebView2のトップレベルドキュメントとして直接embed URLを
開く想定だった。実装・実行したところ **これは動作しないことが判明** し、以下の方式に変更した:

- **問題**: トップレベルnavigationで直接 `/embed/live_stream?channel=...` を開くと、3チャンネル全てで
  即座に「エラー153 動画プレーヤーの設定エラー」。`curl`で同URLを直接取得しても
  `errorCode":"PLAYABILITY_ERROR_CODE_EMBEDDER_IDENTITY_MISSING_REFERRER"` が返り、
  **WebView2固有の問題ではなくRefererヘッダ不在が原因**と判明（トップレベルnavigationやHTML文字列直読み
  (`NavigateToString`, opaque origin)では実URLのRefererが送られない）。
- **修正**: 127.0.0.1のローカルHTTPサーバー（`tiny_http`を追加依存）から配信した親HTML内に
  `<iframe referrerpolicy="strict-origin-when-cross-origin" src="...embed/live_stream?channel=...">`
  として埋め込み、実URLのRefererを送らせる形に変更。これでエラー153は解消した。

このため実装は「WebView2の使い方の問題」ではなく「実サイトが埋め込む形（親ページ+iframe+実Referer）を
ローカルHTTPで再現する」必要があった。本実装に進む場合もこの構成（トップレベル直navigateではなく
親ページ+iframe経由）を踏襲する必要がある。

**もう1点、計画にない追加**: iframeのsrcに `autoplay=1` を付けた（これも計画時点では未決定・実装者が
追加した判断）。付けない場合、プレーヤーはロードされるだけで再生要求が発生せず、サムネイル+関連動画の
静止オーバーレイが表示されて終わる。これでは「映像が実際に再生されるか」を判定できないため、実際に
再生を要求する信号を発生させる目的で追加した（`mute=1`は自動再生ポリシー対応の付随パラメータで、
判定への影響は無い）。この結果、3チャンネル全てで即座にbot認証ゲートが表示される、という
「6. 検証結果」の白黒判定が得られた。

## 6. 検証結果（2026-07-09実施）

| チャンネル | 結果 |
|---|---|
| Al Jazeera English | 黒: bot認証ゲートで詰まる |
| NASA | 黒: bot認証ゲートで詰まる |
| Lofi Girl | 黒: bot認証ゲートで詰まる |

エラー153解消後、`autoplay=1&mute=1`を付けて実際に再生を要求すると、**3チャンネル全てで起動直後の
1枚目のスクリーンショットから即座に「ログインしてbotではないことを確認してください」の認証ゲートが表示
され、再生に至らなかった**（時間経過による解消なし）。プレーヤーのロード自体（埋め込み許可）は成立して
おり、詰まるのは「匿名(未ログイン)セッションでの実際の再生要求」の段階。

**留保**: 本検証実行環境のIPアドレス起因の可能性を排除できていない（データセンター/クラウドIPだと
bot判定が厳しく出ている可能性がある）。一般的な住宅回線のクライアントPCで同じ結果になるかは未確認。

詳細は issue #16 のコメントを参照。

## 6.5 追加検証: Googleログイン済みcookieでのbot認証ゲート回避（2026-07-09実施）

上記「黒」判定を受け、`login` サブコマンドを追加。WebView2の**既定ユーザーデータフォルダ**
（exeのパスに紐づき、プロセスをまたいで永続する）でGoogleアカウントに実際にログインしてもらい
（認証情報の入力は本人が手動で実施。合成入力による自動化はしていない）、その状態のまま同じプロファイル
で通常の埋め込みテストを再実行した。

| チャンネル | 結果 |
|---|---|
| Al Jazeera English | **白**: bot認証ゲートを回避し、実際にライブ映像が再生された（フレーム間で内容変化を確認） |
| NASA | 判定不能: 検証時点でライブ配信自体が終了していた（`/live`の`"style":"LIVE"`が消失）。bot認証ゲートとは無関係 |
| Lofi Girl | 判定不能: ライブ配信中にもかかわらず「この動画は再生できません」という別種のエラー（bot認証ゲートのメッセージではない）。2回再現 |

**結論**: 少なくとも1チャンネル(AJE)で、ログイン済みcookieによるbot認証ゲートの回避を実証できた。
NASA/Lofi Girlの不成立は、bot認証ゲートの再発ではなく別要因（配信終了／個別動画のエラー）であり、
今回の検証で「ログイン済みセッションなら埋め込みiframeがbot認証ゲートを通過できる」という仮説を
否定する材料は出ていない。

**副産物・留意点**:
- 画面BitBltでのスクリーンショットは、撮影直前に自ウィンドウを前面化(`SetForegroundWindow`)する処理を
  入れても、全画面系の他アプリに阻まれて自ウィンドウが写らないことがあった。この際、ユーザーの別アプリ
  （ゲーム・ChatGPTの会話画面）の内容が誤って撮影・保存される事故が発生し、該当ファイルは即座に削除した。
  BitBlt方式は「ユーザーが他アプリを操作していない状態」でのみ信頼できる（既存の
  [docs/features/devtools.md](../docs/features/devtools.md) と同じ制約）。

## 5. スコープ外（このPoCではやらない）

- mpv との経路切替配線
- UI統合（アプリ本体からの起動導線）
- 「YouTubeで開く」フォールバックの実装
- ログイン・年齢制限・メンバー限定ライブへの対応
- 既存 `src/devtools.rs` のリファクタリング・共通化
