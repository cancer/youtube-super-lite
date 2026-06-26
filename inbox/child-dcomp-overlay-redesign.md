# 計画: オーバーレイUI層の再設計（子窓 + DirectComposition）

> inbox の位置づけ: 未着手の計画メモ。確定要件ではない。
> 関連: [opengl-to-native-migration.md](opengl-to-native-migration.md)（P0–P3 で現行ネイティブ版を構築済み）

## 背景 / 動機

現行のネイティブUIは「**mpv 埋め込みの親窓**」＋「**WS_EX_LAYERED の独立トップレベル透過窓**（ULW で per-pixel alpha 合成）」の**2枚のトップレベル窓**で構成されている（[native_overlay.rs](../src/native_overlay.rs)）。UI と動画の描画サーフェスを分けること自体は正しい（毎フレーム合成の無駄を避ける）が、**2枚の対等な窓を手作業で重ね続けている**のが構造的負債:

- `follow_wndproc` で親窓を**サブクラス化**し `WM_MOVE`/`WM_WINDOWPOSCHANGED` を盗み見て `SetWindowPos` で追従（[native_overlay.rs:1607-1636](../src/native_overlay.rs)）
- スクリーン⇔クライアント座標変換が随所に必要
- `WM_MOUSEACTIVATE`→`MA_NOACTIVATE` 等、別窓がアクティブ化して親のフォーカスを奪わないための回避策
- 「画面外でマウスが動くと UI が出る」バグの遠因（グローバル `GetCursorPos` で自窓上かを推測していた。※イベント駆動化で対症修正済み）

### 当時 DComp を見送った前提が崩れた

移行メモ P2 では「libmpv2 の render API は GL/SW のみで、**mpv の D3D11 出力を DirectComposition の visual へ直接バインドする公開 API が無い**」ため ULW 別窓を採用した、と記録されている。
しかし**バインドは不要**だった。mpv は親窓に `wid` 埋め込みのまま据え置き、**DComp を子窓として上に重ねて DWM に合成させれば**よい。これを実機 probe で実証した（下記）。

## 確定した検証事実（`src/bin/child_dcomp_probe.rs`、実機 BitBlt 読み出し）

mpv テストパターン(`av://lavfi:testsrc`)を親窓に埋め込み、`WS_CHILD` + DirectComposition の半透明バーを重ねて画面合成結果を読んだ:

| 確認項目 | 実測 | 判定 |
|---|---|---|
| DComp 半透明UIが mpv の D3D11 flip-model swapchain 上に合成されるか | バー中心 BGR=(213,143,75) 青優勢 | ✅ |
| 透明部から下の動画が透けるか | 動画域 BGR=(45,255,255) testsrc 色（黒/デスクトップでない） | ✅ |
| 親移動に**追従コードなし**で自動追従するか | 親を(+220,+140)移動・子窓無操作→移動後もバー同色(213,143,75) | ✅ |

→ 子窓化＋DComp で「描画分離（効率）」を保ったまま「位置合わせの誤魔化し」を排除できることが確定。

## 目標アーキテクチャ

- **動画**: 現行どおり mpv を winit 親窓に `wid` 埋め込み（`vo=gpu-next`, `gpu-api=d3d11`）。無改修。
- **UI**: winit 親窓の **`WS_CHILD` 子窓**。合成は **DirectComposition**（D3D11→DXGI→DComp device/target/visual/surface、描画は D2D device context）。per-pixel alpha は DComp サーフェスで維持。
- **位置/クリップ/移動**: OS の親子窓関係に一任。`follow_wndproc` サブクラス・`SetWindowPos` 同期・座標変換を**全廃**。リサイズだけ winit の `Resized` で子窓を `MoveWindow`（親 wndproc を盗まない通常処理）。
- **描画タイミング**: 変化時のみ描画＋Commit（毎フレーム合成にしない）。

## 作り方の原則: 核（クリティカル）はゼロベース、無関係モジュールは流用OK

軸は「**今回のリライトの核に関わるか**」。
- **核（クリティカル）= ゼロベース**: 今回作り直す対象＝ウィンドウ/合成/入力/オーバーレイ。バグと負債の温床。旧コードを足場にせず白紙から起こす（描画コマンドも移植せず新規に書く）。
- **無関係・自己完結モジュール = 流用OK（原則無改修）**: 今回のリライトと無関係で、外部契約が安定し動作実績のあるもの。クリーンゲートで足止めしない（死蔵整理は別タスク扱い）。

| 区分 | 対象 | 扱い |
|---|---|---|
| **ゼロベース（核）** | `native_overlay.rs` 全部、`native_app.rs` のウィンドウ生成/DComp 合成/入力/オーバーレイ統合、オーバーレイの D2D 描画（バー/シーク/音量/チャット/一覧/サムネ/URLバー/状態ラベル）、レイアウト、ヒットテスト、`OverlayAction` 設計 | 白紙から実装。旧 `native_overlay` を参照はしても**コピー改変しない** |
| **流用OK（無関係・自己完結）** | `auth`(OAuth)、proxy（wake proxy / backend URL 経路）、`resolve/*`、`chat`、`settings`、`history`、`playlist`、`subscriptions`、`recommend`、`mark_watched`、`image_cache`、`gpu_usage`、`devtools` | 原則無改修で流用。新 UI から呼ぶだけ |
| **要分類（推奨: 流用）** | `controller`（UI非依存オーケストレーション）、`player`（mpv ラッパ／wid 受け渡し） | 核ではない（ウィンドウ/合成に非依存）ので流用を推奨。新 UI はこの2つの既存契約越しにロジックを駆動 |

> 死蔵コード（例: `nsig.rs` の未使用 JsEngine/STUBS/WRAPPER、`clients.rs` の `TVHTML5`）は流用モジュール側の問題で、**今回のリライトの scope 外**。やるなら別タスクの棚卸しで（ここで巻き込まない）。

## 詳細設計

### 1. 合成パイプライン（probe で確立済み）
- `D3D11CreateDevice(BGRA_SUPPORT)` → `IDXGIDevice`
- `DCompositionCreateDevice(dxgi)` → `IDCompositionDevice`
- `CreateTargetForHwnd(child, topmost=true)` → target、`CreateVisual` → root visual、`target.SetRoot(visual)`
- `CreateSurface(w,h,B8G8R8A8_UNORM,PREMULTIPLIED)` → visual.SetContent
- 描画: `surface.BeginDraw()`→DXGIサーフェス＋offset→`CreateBitmapFromDxgiSurface`→`d2d_ctx.SetTarget`→既存の D2D 描画→`EndDraw`→`surface.EndDraw`→`dcomp.Commit`
- **device-lost 復旧**: `Commit`/`BeginDraw` が `DXGI_ERROR_DEVICE_REMOVED` を返したらデバイス一式を作り直す経路を用意（現行 ULW には無い新規考慮点）。

### 2. 入力（子窓が全入力を所有。貫通はしない）
- オーバーレイ子窓を最前面に置き、**全クライアント領域を `HTCLIENT`**（既定動作）で受ける。`HTTRANSPARENT` 貫通はしない。
- クリックを領域で振り分け: コントロール帯/一覧/チャット → 各操作、**動画域 → 自前で `TogglePause` 相当**に変換。いずれも `OverlayAction` に積み `native_app` が drain。
- **mpv へ入力を通す必要はない**（mpv はコマンド駆動でマウス不要）。よって mpv が wid 内に作る出力子窓（`class="mpv"`、d3d11 で常に生成される）の z-order や貫通は**入力経路に無関係**。オーバーレイ子窓が上に居て全入力を取ればよい。
- 自動表示は子窓の `WM_MOUSEMOVE` を活動源にする（動画域・帯を問わず子窓が受けるので、現行の「winit `CursorMoved` ＋ overlay `take_moved`」の二系統を**子窓1系統に統一**できる）。グローバル座標は不要。
- キーボードは winit 側に残す: 子窓がアクティブ化でフォーカスを奪わないよう `WS_EX_NOACTIVATE`／`WM_MOUSEACTIVATE`→`MA_NOACTIVATE` 相当で非アクティブ受領にする。

### 3. アクティブ化/フォーカス
- 子窓は独立してアクティブ化しない → `WM_MOUSEACTIVATE`/`MA_NOACTIVATE` の回避策と、`focused` 依存のオーバーレイ可視制御の特別扱いを**削減**できる見込み（パリティ確認で詰める）。

### 4. devtools スクショ
- 現行は `SetForegroundWindow`＋画面 BitBlt。probe で **DComp 合成結果も画面 BitBlt で読める**ことを確認済み。スクショ経路は維持可能。ついでにフォーカス奪取（`SetForegroundWindow`）の要否を見直す。

## 進捗

- ✅ **手順0 凍結**: tag `legacy-ulw-overlay` ＋ ブランチ `redesign/child-dcomp-overlay`。
- ✅ **手順1 ホスト骨組み**: [src/dcomp_overlay.rs](../src/dcomp_overlay.rs) 新規（ゼロベース）。子窓(`WS_CHILD`)＋D3D11→DXGI→DComp/D2D＋サーフェス＋プレースホルダ描画＋全入力受領（`GWLP_USERDATA`、thread_local 不使用）。`native_app` に `--dcomp` トグルで排他統合（旧 ULW は既定で温存）。実アプリ `--dcomp` 起動で初期化完走・クラッシュなし・新コード警告ゼロを確認。動画域クリック→`TogglePause` 配線済み。
- 🔄 **手順2 描画移植（進行中）**:
  - ✅ 2a コントローラ帯コア: 半透明帯・シークライン(track/progress/knob)・再生/一時停止グリフ・時間表示・音量バーを DirectWrite/D2D で新規描画。ヒットテスト（pause/seek/volume）＋ドラッグ（seek/vol キャプチャ）＋ホイール音量＋3秒自動非表示。`--dcomp` 実アプリで devtools スクショ視覚確認済み（レイアウトは旧 draw_controller=egui 踏襲と一致）。dcomp 経路でも /screenshot が効くよう capture を if/else 外へ移動。
  - ⬜ 2b 以降: ミュート/画質/コーデック/Like、上部バー(URL/認証/タイトル)、一覧(4ソース＋サムネ＋クリック)、チャット(左右分割＋スクロール＋幅ドラッグ)、ライブ最新ボタン。
- ⬜ 手順3 以降（下記）。

## 移行手順（各段階でアプリは動く状態を保つ）

0. **凍結**: 現状を git tag（例 `legacy-ulw-overlay`）で固定。新ブランチで作業。旧 ULW 版はパリティ達成まで温存。
1. **DComp ホスト骨組み**: 子窓＋デバイス＋サーフェス＋Commit を実装。空オーバーレイが動画上で透過することを確認。
2. **描画移植**: 既存 `render()` の D2D 描画コマンドを DComp サーフェス描画へ移植（バー→シーク/音量→チャット→一覧→サムネ→URLバー→各状態ラベル）。
3. **入力移植**: `WM_NCHITTEST`/`WM_LBUTTONDOWN`/ドラッグ/ホイールを子窓 wndproc に移植。動画クリックが親へ抜けることを実機確認。
4. **負債撤去**: `follow_wndproc` サブクラス・`SetWindowPos` 追従・座標変換・アクティブ化回避を削除。リサイズは winit `Resized` 経由に統一。
5. **パリティ検証**（下記チェックリスト）。
6. **切替**: `main` を新版へ。旧 `native_overlay.rs`（ULW）削除（tag に残る）。
7. **撤去（必須・別途にしない）**: 下記「撤去対象インベントリ」を全消化してから完了とする。

## 原則: ゼロ・レフトオーバー

この遠回り（カーソルバグ調査 → 二窓構成の問題特定 → DComp 実証）で生まれた**使い捨てコードを最終状態に残さない**。
履歴は git tag / commit に残るので、ファイルとして残す必要はない。移行中の一時的な並存（旧ULWを温存して動作維持）は可。ただし**完了時点で重複・死蔵コードがゼロ**であることを完了条件とする。

## 撤去対象インベントリ

| 対象 | 由来 | 撤去タイミング | 状態 |
|---|---|---|---|
| `target-verify/`（野良ビルド出力・未追跡） | 検証時の別 target dir | 即 | ✅ 削除済み（.gitignore 追加済み） |
| `src/bin/mpv_d3d11_probe.rs`（P1 実証） | OpenGL→native 移行（P3 で完了） | 即〜本作業中 | 死蔵候補 |
| `src/bin/overlay_probe.rs`（P2 GDI 実証） | 同上 | 即〜本作業中 | 死蔵候補 |
| `src/bin/d2d_overlay_probe.rs`（P3a/b D2D 実証） | 同上 | 即〜本作業中 | 死蔵候補 |
| `src/bin/child_dcomp_probe.rs`（本件 DComp 実証） | このセッション | — | ✅ 削除済み（合成・透過・自動追従を実証して役目完了） |
| 旧 `native_overlay.rs`（ULW 別窓・follow） | 現行 | 手順6 切替時 | パリティ達成まで温存 |
| 移行中の一時ファイル `native_overlay_dcomp.rs`(仮) | 本作業 | 完了時に正式名へ統合（並存解消） | — |
| Cargo features の probe 用コメント / 不要 feature | 各 probe | 各 probe 削除に合わせ整理（DComp/D3D11/Dxgi は製品が使うので残す） | — |

> 注: `u1_player_probe.rs` / `u5_sig_probe.rs` / `u7_auth_probe.rs` は解決器・認証ドメインの probe で本UI作業の対象外。
> 純Rustネイティブ解決器への置換が済んでいるため別途の棚卸し候補だが、本計画では触らない（別タスク）。

## パリティ・チェックリスト（切替ゲート）

- [ ] 再生（`scripts/verify_playback.ps1` で AV 時計前進）
- [ ] コントローラ表示・自動非表示（3秒）・カーソル/操作で再表示
- [ ] **画面外/別窓上のマウス移動で UI が出ない**（今回の発端バグ）
- [ ] シーク/音量ドラッグ（領域外追従含む）
- [ ] 動画クリック=pause が子窓を貫通して効く
- [ ] チャット左右分割・スクロール・幅ドラッグ
- [ ] 一覧（登録/おすすめ/履歴/再生リスト）表示・選択・クリック・サムネ
- [ ] URL 入力・Ctrl+V 貼り付け
- [ ] ログイン(Ctrl+L)/Like/画質/コーデック
- [ ] devtools スクショ（`--enable-dev-tools` /screenshot）
- [ ] ウィンドウ移動・リサイズで UI が追従（追従コードなし）
- [ ] 複数アプリ窓同時起動で各窓が独立動作
- [ ] device-lost（GPU ドライバ更新/スリープ復帰）でクラッシュせず復旧

## リスク / 未確認点

- ~~子窓 `HTTRANSPARENT` の入力貫通~~ → **解消**。新設計は貫通せず子窓が全入力を所有するため、mpv 子窓との z-order 問題自体が発生しない（旧設計由来の phantom 要件だった）。
- **device-lost 復旧**: ULW には無かった新規考慮。設計に織り込み済みだが実装・試験が要る。
- **キーボードフォーカス**: 子窓が活動化で winit からフォーカスを奪わないこと（`WS_EX_NOACTIVATE` 等）。実装で対応。
- **z-order 維持**: オーバーレイ子窓が mpv 出力子窓の上を保つこと（後から生成で上になるが、リサイズ等で再確認）。
- **DPI/マルチモニタ**: DComp サーフェスサイズと子窓物理ピクセルの整合。probe は等倍前提。スケーリング環境で要確認。
- **WS_CLIPCHILDREN/WS_CLIPSIBLINGS**: 親の mpv 描画と子窓のクリップ相互作用。probe では合成成立を確認済みだが本番レイアウトで再確認。

## 見積り（粗）

ホスト層の載せ替えが主で、描画ロジックとビジネスロジックは流用のため、**新規実装は合成パイプライン＋子窓入力＋device-lost**に集中。最大の不確実性は「子窓入力貫通」で、ここを手順1–3の早期に潰せば見通しが立つ。
