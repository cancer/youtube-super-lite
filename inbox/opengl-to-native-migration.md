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
  - 残り P3 本体（数週規模）: URL欄+IME(DirectWrite/TSF)、タイトル/その他コントロール、
    一覧系のサムネグリッド仮想化（image_cache→WICデコード）、チャット左右分割。
    Controller(P0) を駆動する実フロントエンド(ネイティブ版エントリ)に統合して順次移植。
- **P4 切替**: 機能同等になったら egui/glutin/glow/egui_glow/gl_quad と OpenGL 経路を削除。
- **P5（後日）mac**: CoreAnimation + mpv `gpu-api=metal`。共有コア再利用。

## リスク / 要検証
- mpv ↔ DirectComposition の正確なバインドは P1 で実証（駄目なら子HWND＋透過レイヤードウィンドウにフォールバック）。
- リッチ UI（サムネグリッド仮想化・IME・チャット）の Direct2D 再実装は相応の工数（数週規模）。
- 合成のグルーはプラットフォーム別（DirectComposition / CoreAnimation）。UI 描画ロジックは共通化できる。
