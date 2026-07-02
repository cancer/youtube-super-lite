# オーバーレイ描画の設計（子窓 + DirectComposition）

対象読者: コントローラ帯・一覧・チャットなど UI 描画まわりを触る人。

## 現行方式

コントローラ・URL欄・一覧・チャットは、本体ウィンドウに重ねた **`WS_CHILD` の子ウィンドウ**に
**DirectComposition** 経由で D3D11 サーフェスを合成し、その上に Direct2D/DirectWrite/WIC で描画する
（`dcomp_overlay` モジュール）。per-pixel alpha はこのコンポジション経路で維持される。

- **入力モデル**: すべてのマウス入力は子窓側で受け取る（クリックスルーはしない）。クリック位置は
  コンポーネントごとの矩形と照合して振り分ける（コントロール領域→対応操作、動画領域→再生/一時停止）。
  キーボードはフォーカスを奪わないよう子窓を `WS_EX_NOACTIVATE` にし、winit の親ウィンドウ側で処理する。
- **描画モデル**: `Control` 列挙型でコンポーネントを表現し、各コントロールが自身の描画範囲とクリック
  ハンドラを持つ。レイアウトは下部コントローラ帯・上部パネル（URL/タイトル/認証）・チャット（右側、条件付き）・
  一覧（全画面、条件付き）で構成される。チャットパネル幅はパネル左端のドラッグでも変更できる。
- **デバイスロスト対応**: `Commit`/`BeginDraw` が `DXGI_ERROR_DEVICE_REMOVED` を返した場合、D3D11 /
  DirectComposition デバイスを丸ごと再生成する。
- **状態管理**: スレッドローカルなグローバル変数は使わず、ウィンドウごとの状態は `GWLP_USERDATA` に
  保持する（複数ウィンドウを想定した設計）。

## 旧方式（撤去済み）とその理由

以前は `WS_EX_LAYERED` の単一レイヤード窓に Direct2D で描画し、`UpdateLayeredWindow(ULW_ALPHA)` で
動画の上に per-pixel alpha 合成する方式だった（旧 `native_overlay.rs`）。この方式には以下の課題があった:

- レイヤード窓は入力をクリックスルーさせる必要があり、`follow_wndproc` によるウィンドウ座標追従・
  アクティブ化回避などの負債コードが必要だった
- （さらに遡ると）当初は mpv の OpenGL Render API + egui を単一 GL コンテキストで合成していたが、
  起動時の OpenGL ドライバ bring-up が他アプリの GPU 再生を一瞬妨げる問題があり、
  「mpv 埋め込み(D3D11) + Direct2D 2D UI」へまず移行した（この移行の記録は
  [inbox/opengl-to-native-migration.md](../../inbox/opengl-to-native-migration.md)）

子窓 + DirectComposition への再設計により、`follow_wndproc`・`SetWindowPos` 追従・座標変換・
アクティブ化回避まわりのコードは撤去された（旧 `native_overlay.rs` を含め `+53/-3433` 行の削減）。
再設計の計画・移行手順の記録は [inbox/child-dcomp-overlay-redesign.md](../../inbox/child-dcomp-overlay-redesign.md)。

## 描画とmpvの分担（再掲）

動画は mpv が D3D11 でウィンドウへ直接描画し、UI は別の透過子窓に DirectComposition で描画する。
両者は GPU コンテキストを共有しない。チャット表示中は mpv の `video-margin-ratio-right` プロパティで
動画の描画領域自体を左に縮め、空いた右側にチャットパネルを描く（オーバーレイの重ね描きではなく、
真の左右分割）。詳細は [architecture-overview.md](architecture-overview.md) を参照。

## 関連

- [../features/controller-ui.md](../features/controller-ui.md) — ユーザーから見た操作・キーバインド
- [../features/chat.md](../features/chat.md) — チャットパネルの左右分割表示
