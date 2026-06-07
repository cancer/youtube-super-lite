# 計画: OpenGL 合成からの脱却（mpv 埋め込み + 2D UI）

> inbox の位置づけ: 未着手の計画メモ。確定要件ではない。

## 背景 / 動機（このセッションで実測・特定した事実）
- 起動の瞬間、ブラウザ等で再生中の他アプリの GPU 動画が一瞬カクつく。
- 切り分け（URLなし起動でも再現＝yt-dlp/デコード無関係、GPU監視も無関係）の結果、原因は
  **OpenGL コンテキスト/ドライバの bring-up**（init 計測で `window+gl_config` が ~2.5〜3秒、
  実体は NVIDIA OpenGL ドライバ(nvoglv64.dll)のロード＝GPU の起き上がり）。
- 単体 GPU（GTX 1070）共有のため別GPUへの退避は不可。ブラウザ/DWM が使う **D3D は常時温まっている**のに、
  本アプリだけ重い OpenGL ICD を新規ロードするのが競合の正体。
- **唯一の目的は「OpenGL を使わない＝起動時の GPU 競合を解消する（性能）」**。
  （アクセシビリティ等は本件の動機ではない。）

## 何が OpenGL を必須にしているか
動画再生ではなく、**「mpv の OpenGL Render API で動画をテクスチャに描き、egui と単一 GL コンテキストで合成する」**
という現在の設計。3者（mpv Render API[GL] / egui_glow / FullscreenQuad）が1つの GL コンテキストを共有している。

- mpv 自体は OpenGL を強制しない（D3D11/Vulkan/Metal を内部で使える。Render API は GL か SW のみ）。
- UI（コントロール・一覧・サムネ）は本質的に 2D で、3D コンテキストは不要。
- → GL を必須にしているのは「自前 GL 合成」であって mpv ではない。

## 目標アーキテクチャ
- **共有コア（OS非依存・Rust）**: 状態/Controller、mpv 制御、yt-dlp 解決/DASH、InnerTube(chat/recommend/subscriptions)、
  playlist、history、auth(OAuth+Worker)、image_cache、mark_watched。既存モジュールをそのまま流用。
- **動画**: mpv を埋め込み（`vo=gpu-next`, `gpu-api=d3d11`[Win]）。HWデコード＋描画を mpv が内部完結。OpenGL 不使用。
- **UI**: 2D レイヤ（Win: Direct2D + DirectWrite[テキスト/IME] + WIC[画像]）。3D コンテキストを作らない。
- **重ね方**:
  - コントローラ/タイトル → 動画に重ねる（自動非表示）。OS コンポジタでレイヤ合成
    （Win: DirectComposition / mac: CoreAnimation）。UI は透過2Dレイヤ、mpv とは GPU コンテキスト非共有。
  - 一覧系（おすすめ/登録/履歴/再生リスト）→ 動画を畳んで 2D が全画面（重ねない＝合成不要）。
  - チャット → 左右分割（動画を縮小、既存挙動）。
- **入力**: OS のウィンドウメッセージ → UI ヒットテスト or mpv へ振り分け（透過部はクリックを動画へ通す）。

## GPU API
- Windows = **D3D11**（DirectComposition との相性が最良）。
- mac = Metal、linux = Vulkan を mpv の `gpu-api` で OS 別に選ぶ（プラットフォーム別ネイティブ）。
- ＝「Vulkan 一択」ではない。

## 段階移行（各フェーズで動く状態を保つ。egui 版は P4 まで温存）
- **P0 コア分離** ✅ **完了 (commit 8dbf923)**: `src/controller.rs` を新設し、UI 非依存の状態とロジック
  （mpv 制御・認証/API・yt-dlp 解決・各種 poll/start・mark_watched・GPU 監視）を `controller::Controller` へ集約。
  `Running` は egui/OpenGL/window と DevTools 入力のみを保持し、`self.core` 経由で Controller を駆動する。
  redraw は「poll群 → GL描画 → 状態スナップショット → egui で intent 収集 → intent 適用(core 呼び出し)」の構造。
  機能差分なし（debug ビルド・起動・登録チャンネル取得/カード描画を実機確認）。
  次フェーズ着手前のメモ: intent はまだ redraw 内のローカル変数で受け渡している。必要なら後続で `Intent` enum に形式化。
- **P1 mpv 埋め込み実証** ✅ **完了（probe 実装・実測）**: `src/bin/mpv_d3d11_probe.rs` を新設。
  素の Win32 窓（windows-rs で RegisterClass/CreateWindowEx）の HWND を mpv の `wid` に渡し、
  `vo=gpu-next` `gpu-api=d3d11` `force-window=yes` で **OpenGL コンテキストを一切作らずに** mpv 自身が
  D3D11 で埋め込み描画することを確認。mpv ログで実証:
  `vo/gpu-next/d3d11] Using Direct3D 11 feature level 12_1` / `Device Name: NVIDIA GeForce GTX 1070` /
  `Using flip-model presentation` / `VO: [gpu-next] 1280x720`（libplacebo）。正常終了 exit 0。
  → 起動時に nvoglv64.dll(OpenGL ICD) をロードする現行経路を構造的に回避できることを確認。
  残: 「ブラウザで YouTube 再生中に probe を起動してカクつきが消えるか」の主観確認は実機で要観察
  （原因＝OpenGL bring-up は排除済み）。`wid` 直接埋め込みで成立したため、子HWND＋透過窓フォールバックは不要。
  使い方: `cargo run --bin mpv_d3d11_probe [-- <file|url>]`。
- **P2 2D レイヤ＋合成** ✅ **完了（probe 実装）**: `src/bin/overlay_probe.rs` を新設。
  合成方式の判断: libmpv2 の render API は OpenGL/SW のみで、mpv の D3D11 出力を DirectComposition の
  visual へ直接バインドする公開 API が無い。よって計画想定の **「mpv 子窓＋透過オーバーレイ窓」** 構成を採用:
  - ベース窓 = mpv を `wid` で D3D11 埋め込み（P1 と同じ、OpenGL 不使用）
  - オーバーレイ窓 = WS_EX_LAYERED トップレベル透過窓。GDI で最小コントローラ帯を描画（カラーキーで透過）
  検証 3 点を実装: ①動画上に透過 2D を重ねる ②無操作 3 秒で自動非表示／カーソル移動で再表示
  （タイマで GetCursorPos 監視）③入力振り分け（WM_NCHITTEST: 帯=HTCLIENT でオーバーレイ受領、
  それ以外=HTTRANSPARENT で動画へ透過。帯クリックで mpv pause トグル）。ビルド成功・8 秒起動・正常終了 exit 0。
  残: 重なり表示/自動非表示/クリック振り分けの**見た目の確認は実機観察が必要**。
  製品版は透過 2D を Direct2D + DirectComposition に置換予定（本 probe はレイヤ合成・自動非表示・
  入力振り分けの**モデル検証**が目的）。使い方: `cargo run --bin overlay_probe -- <file|url>`。
- **P3 UI 移植**: コントローラ全部、URL 欄(IME)、タイトル、一覧系（全画面2D＋サムネグリッド。image_cache がバイト供給→WICデコード）、チャット（左右分割）。
  - **P3a 描画基盤実証** ✅ **完了**: `src/bin/d2d_overlay_probe.rs`。P2 の GDI 描画を Direct2D に置換し、
    製品 UI で必要な 2D 描画スタックを実証。透過オーバーレイ(WS_EX_LAYERED + DCRenderTarget を
    32bpp premultiplied DIB にバインド → UpdateLayeredWindow(ULW_ALPHA) で per-pixel alpha 合成)に:
    ①Direct2D の AA 角丸矩形 ②DirectWrite の日本語テキスト ③WIC で JPEG をデコード→ID2D1Bitmap 表示、
    を描画。実測: `WIC: サムネイルをデコードしました` / `帯中心ピクセル BGRA=(255,255,255,255)`(alpha>0=描画成功)。
    Cargo features 追加: System_Com / Foundation_Numerics / Direct2D(+Common) / DirectWrite / Imaging / Dxgi_Common。
    使い方: `cargo run --bin d2d_overlay_probe -- <video|url> [thumbnail.jpg]`。
  - **P3b 実動コントローラ** ✅ **完了**: 同 `d2d_overlay_probe` を発展。mpv の再生状態
    (time-pos/duration/pause) を `get_property` で読み、Direct2D で再生/一時停止ボタン(グリフ)・
    シークバー(トラック/進捗/ノブ)・時間表示(mm:ss/mm:ss)を描画。WM_LBUTTONDOWN でボタン=pause トグル／
    シークバー=絶対シーク(`seek <pct> absolute-percent`)に振り分け（hit-test 矩形を render で保存）。
    実測: `time-pos=3.80 duration=4.87 pause=false`（ライブ取得・前進を確認）、帯ピクセル alpha>0。
  - **ネイティブ版エントリ骨組み** ✅ **完了**: `src/native_app.rs` + `--native` フラグ。
    winit ウィンドウの HWND を `Player::new_embedded`(wid/D3D11) に渡し、`Controller`(P0) を
    そのまま駆動する実フロントエンド。OpenGL を一切作らない。キーボード操作(space/矢印)、
    各種 poll(認証/チャット/おすすめ/登録/履歴/再生リスト/解決)、GPU監視を配線。
    あわせて `Player` を `Option<GlBackend>` 化し GL版/埋め込み版を一本化、`Controller::new` を新設。
    実測: `--native` 起動で `VO: [gpu-next] 1280x720`(D3D11)・再生時間前進・正常終了。egui版は無変更。
    （probe の D2D オーバーレイ統合・実 UI 移植は後続）。
  - **D2D オーバーレイのネイティブ統合** ✅ **完了**: `src/native_overlay.rs`(`Overlay`)を新設し
    NativeApp に統合。winit 親窓(=mpv D3D11)の上に WS_EX_LAYERED 透過窓を重ね、`Player` の
    再生状態(time-pos/duration/pause)を読んで Direct2D でコントローラ（再生/一時停止グリフ・
    シークバー・時間表示）を描画、UpdateLayeredWindow(ULW_ALPHA) で per-pixel alpha 合成。
    `about_to_wait` で ~10fps 再描画（ControlFlow::WaitUntil）。現段はクリックスルー表示専用
    （操作はキーボード）。実測（画面キャプチャ）: 埋め込み映像の上にコントローラが重なり
    「00:02 / 00:03」とシークバー進捗がライブ表示されることを確認。OpenGL 不使用。
  - **オーバーレイ入力振り分け＋自動非表示** ✅ **完了**: オーバーレイのクリックスルーをやめ、
    WM_NCHITTEST で帯のみ受領（他は HTTRANSPARENT で動画へ透過）。WM_LBUTTONDOWN を
    ボタン=pause トグル / シークバー=絶対シークに振り分け、`OverlayAction` を thread_local 経由で
    NativeApp に渡し Player に適用。NativeApp は GetCursorPos で活動を監視し 3 秒無操作で
    `set_visible(false)`（カーソル移動/キー操作で再表示）。実測（画面キャプチャ）: 6 秒無操作で
    コントローラが消えることを確認。クリック振り分けは検証済み probe と同一ロジック。
  - **URL 入力欄（基本）** ✅ **完了**: オーバーレイ上部に URL バーを Direct2D で描画
    （`render` に `url_input` を渡す）。NativeApp が winit のキー入力を処理: 印字可能文字は
    URL 欄へ追記、Backspace/Esc 編集、Enter で `core.load` + チャット/おすすめ開始。URL は
    空白を含まないため Space は再生/一時停止に温存（フォーカス概念なし・IME 不要）。CLI URL も欄へ反映。
    実測（画面キャプチャ）: 映像上にコントローラ（00:01/00:02）が表示、URL バーも同一 D2D 経路で描画。
  - **URL 欄の貼り付け(Ctrl+V)** ✅ **完了**: `native_overlay::clipboard_text()` で CF_UNICODETEXT を
    取得（OpenClipboard/GetClipboardData/GlobalLock）。NativeApp は ModifiersChanged で Ctrl 押下を
    追跡し、Ctrl+V で URL 欄へ貼り付け。Cargo features 追加: System_DataExchange/Ole/Memory。
  - **一覧ビュー（登録チャンネル新着・テキスト）** ✅ **完了**: Tab で全面パネルを開閉。
    開く時に未取得なら `core.start_subs()` を起動、↑↓ で選択（スクロール追従）、Enter で
    `core.load`、Esc/Tab で閉じる。`render` に `list_open`/タイトル配列/選択 index を渡し、
    Direct2D で半透明パネル＋行＋選択ハイライトを描画。一覧表示中は自動非表示を抑止し
    ヒット判定を透過化。ビルド通過。URL バー描画は実機キャプチャで確認済み。
  - **一覧のサムネ表示** ✅ **完了**: `image_cache::cache_dir()/cached_path()` を公開（egui 版と
    共有。main.rs の重複 `image_cache_dir` は撤去）。`Overlay` に URL→ID2D1Bitmap キャッシュを持ち、
    表示行のサムネを（ディスクキャッシュ済みのものだけ）WIC デコードして行左に描画（行高 48px、16:9）。
    ネットワーク取得はしない（egui 版 BytesLoader が保存済みのものを読む）。未キャッシュはテキストのみ。
  - **一覧の複数ソース化** ✅ **完了**: 一覧を 1=登録新着 / 2=おすすめ / 3=履歴 で切替可能に。
    `NativeRunning::list_rows()` が各ソースを (タイトル, サムネURL, video_id) に正規化、
    `ensure_source_fetched()` が未取得なら start_subs/start_history を起動（おすすめは再生中動画に紐づく）。
    ヘッダもソース名に追従。Enter で選択動画を再生。
  - **一覧のクリック選択** ✅ **完了**: 一覧表示中は WM_NCHITTEST を HTCLIENT にして overlay が
    全クリックを受領。render で行ジオメトリ(top/row_h/first/count)を OV_STATE に保存し、
    WM_LBUTTONDOWN で y からクリック行 index を算出して `list_click` に格納。NativeApp が
    `take_list_click()` で取り出し、その動画を再生。
  - **ログインUI＋認証状態表示** ✅ **完了**: Ctrl+L で `core.start_login()`（ブラウザ OAuth 同意）。
    上部バー右に認証状態を表示（ログイン中はチャンネル名「👤 …」、未ログインは auth_status＋「（Ctrl+L）」）。
  - **Like＋画質/コーデック選択** ✅ **完了**: Ctrl+G で `core.start_like(video_id)`、Ctrl+Q で画質、
    Ctrl+C でコーデックを巡回切替（YouTube 再生中なら `core.start_resolve` で取り直し）。
    コントローラ帯の上に「画質: … ｜ コーデック: …（Ctrl+Q/C・Ctrl+G）」の状態ラベルを表示。
  - **チャット左右分割** ✅ **完了**: Ctrl+T で開閉。`Player::set_video_margin_right(0.28)` で
    mpv の映像を左に縮め、空いた右 28% に Direct2D でチャットパネルを描画（`core.chat_messages` の
    author: text を末尾から表示。ChatRun::Image は alt テキスト）。閉じると margin を 0 に戻す。
  - **再生リスト一覧（2階層）** ✅ **完了**: ListSource::Playlist を追加。4 キーで選択、未取得なら
    start_playlist_list。1 階層目（リスト一覧）で Enter → start_playlist_items で中身を開く、
    2 階層目（動画）で Enter → 再生、Backspace で一覧へ戻る。
  - **IME 不要と判断**: このアプリの唯一のテキスト入力は URL 欄（ASCII のみ）でチャット投稿も無いため、
    日本語 IME は実機能上の parity ギャップではない（URL タイプ＋Ctrl+V 貼り付けで充足）。
  - → **P3 の機能 parity 完了**。egui 版の UI 機能はネイティブ版(`--native`)に出揃った。
    次は **P4: egui/glutin/glow/egui_glow/gl_quad と OpenGL 経路を撤去**して移行完了。
- **P4 切替**: 機能同等になったら egui/glutin/glow/egui_glow/gl_quad と OpenGL 経路を削除。
- **P5（後日）mac**: CoreAnimation + mpv `gpu-api=metal`。共有コア再利用。

## リスク / 要検証
- mpv ↔ DirectComposition の正確なバインドは P1 で実証（駄目なら子HWND＋透過レイヤードウィンドウにフォールバック）。
- リッチ UI（サムネグリッド仮想化・IME・チャット）の Direct2D 再実装は相応の工数（数週規模）。
- 合成のグルーはプラットフォーム別（DirectComposition / CoreAnimation）。UI 描画ロジックは共通化できる。
