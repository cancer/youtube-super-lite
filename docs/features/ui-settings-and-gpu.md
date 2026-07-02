# UI設定の永続化・画像キャッシュ・GPU負荷監視

対象読者: チャットの見た目設定の保存先、サムネキャッシュの仕組み、HW/SWデコード自動切替の挙動を確認したい人。

## UI設定の永続化（`settings.rs`）

チャットの文字サイズ・パネル幅は次のパスに JSON で保存され、次回起動時に引き継がれる:

- Windows: `%APPDATA%\YouTubeSuperLite\settings.json`（トークン保存先と同じディレクトリ）

| 項目 | クランプ範囲 | 既定値 |
|------|------|------|
| `chat_font_px` | 10.0〜28.0 | 16.0 |
| `chat_width_ratio` | 0.15〜0.6 | 0.28 |

## 画像キャッシュ（`image_cache.rs`）

サムネイルを自前で取得し、OS のキャッシュ領域にディスクキャッシュする。

- キャッシュ先: `%LOCALAPPDATA%\YouTubeSuperLite\image-cache`
- ファイル名: URL を **FNV-1a 64bit** でハッシュ化した16桁の16進数
- 同一URLの二重ダウンロードは進行中セットで防止する
- 書き込みは `.tmp` へ書いてからリネームするアトミック方式
- デコードは描画時に WIC（Windows Imaging Component）が行う。キャッシュ層はバイト列の用意のみ担当する

## GPU負荷監視と自動HW/SWデコード切替（`gpu_usage.rs`）

Windows の PDH カウンタ `\GPU Engine(*)\Utilization Percentage` を 1 秒間隔でポーリングし、GPU使用率
（すべてのエンジン種別を種別ごとに合算し最大値を取ることで二重カウントを防ぐ）を監視する。

- 使用率が **80%** を超える状態が一定時間（3秒）続く → ソフトウェアデコードへ切替（`hwdec=no`）
- 使用率が **60%** を下回る状態が一定時間（5秒）続く → ハードウェアデコードへ復帰（`hwdec=auto`）

閾値を跨いだ瞬間ではなく一定のホールド時間を要求することで、切り替えのバタつき（ヒステリシス）を防ぐ。
Windows以外ではこの監視自体が無効（`None`）になる。

## 関連

- [chat.md](chat.md) — チャットのフォントサイズ/幅の操作
- [playback.md](playback.md) — HW/SWデコード切替が再生に与える影響
- [browse-lists.md](browse-lists.md) — サムネキャッシュを使う一覧機能
