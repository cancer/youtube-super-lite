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
- **P0 コア分離**: main.rs の描画(egui/GL)とロジック(intent/poll/load等)を分離し、UI 非依存の Controller を切り出す。
- **P1 mpv 埋め込み実証**: 素の Win32 窓に mpv(D3D11)で再生。**OpenGL を一切作らずに再生でき、起動時に他アプリが
  カクつかない**ことを実測（本移行の核の検証）。最悪「mpv 子窓＋透過オーバーレイ窓」でも成立。
- **P2 2D レイヤ＋合成**: DirectComposition で「動画視覚＋透過2D視覚」。最小コントローラを重ねて自動非表示＋入力振り分け検証。
- **P3 UI 移植**: コントローラ全部、URL 欄(IME)、タイトル、一覧系（全画面2D＋サムネグリッド。image_cache がバイト供給→WICデコード）、チャット（左右分割）。
- **P4 切替**: 機能同等になったら egui/glutin/glow/egui_glow/gl_quad と OpenGL 経路を削除。
- **P5（後日）mac**: CoreAnimation + mpv `gpu-api=metal`。共有コア再利用。

## リスク / 要検証
- mpv ↔ DirectComposition の正確なバインドは P1 で実証（駄目なら子HWND＋透過レイヤードウィンドウにフォールバック）。
- リッチ UI（サムネグリッド仮想化・IME・チャット）の Direct2D 再実装は相応の工数（数週規模）。
- 合成のグルーはプラットフォーム別（DirectComposition / CoreAnimation）。UI 描画ロジックは共通化できる。
