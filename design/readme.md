# YouTube Super Lite — Design System

ダークテーマの動画視聴 UI デザインシステム。トークン・コンポーネント・UI キットを収める。

このデザインシステムは **本プロジェクト配下** に保存されている（専用のデザインシステムリポジトリは作らない）。

> **状態**: このデザインシステムは、削除済みのネイティブ実装（Rust + mpv + Direct2D オーバーレイ）の
> UI を体系化したものとして作られた。現在プロジェクトは Chrome 拡張として再構成中であり、
> **どこまで流用するかは未定**。トークンとコンテンツ原則は方式に依存しないが、
> オーバーレイ前提のコンポーネント（コントローラ帯・一覧オーバーレイ）は廃棄対象の UI にあたる。
> 経緯は [../CLAUDE.md](../CLAUDE.md) と要件文書を参照。

## トークン構成（2 層）
- **プリミティブ `--p-*`** — 色・寸法・字形の生値。意味を持たない。`tokens/primitives.css`。
- **セマンティック `--s-*`** — プリミティブを用途で束ねた参照。**UI 実装はこの層のみ参照する**。`tokens/semantic.css`。
- フォント: `tokens/fonts.css`（Roboto + Noto Sans JP を Google Fonts CDN から。Yu Gothic UI / Meiryo は Windows システムフォント）。
- エントリ: `styles.css`（上記を @import するだけ）。

## コンテンツの原則
- **日本語ファースト**。ラベルは短い名詞句（ホーム / 登録チャンネル / 再生履歴 / 再生リスト / もっと見る / 並び替え）。
- **数値表記**: 視聴回数は「4.3万回視聴」、配信中は「1.2万人が視聴中」。時間バッジは "1:11:13"。経過時間は「11 分前」「3 時間前」。区切りは中黒「・」。
- **トーン**: 機能的で淡々。装飾は最小限。絵文字はチャットのメンバーバッジ等、ユーザー由来の文脈のみ。

## ビジュアル基礎
- **ダーク専用**。地 `--s-bg-canvas`(#0F0F0F) → 面 `--s-bg-surface`(#212121) → 浮き `--s-bg-elevated`(#272727) → 選択 `--s-bg-selected`(#3F3F3F) の **明度差で前後関係を表現**（影は基本使わない）。
- **色は赤のみ**。アクセントは LIVE `#CC0000` / ブランド・通知 `#FF0000` に限定。それ以外の彩度はサムネイル画像から来る。
- **タイポ**: Roboto + Noto Sans JP。スケールは 36/24/20/16/14/12px（各行高あり）。役割別は `--s-type-*`（page-title=3xl/700, section=xl/700, card-title=md/500, meta=sm/400 など）。
- **角丸（形の役割で束ねる）**: overlay 4（時間/LIVE バッジ）・soft 8（チップ/ナビ行ホバー）・container 12（カード/サムネ）・pill（ボタン）・circle（アバター/ドット）。
- **カード**: 16:9 サムネ + 本文（アバター + 2 行タイトル + チャンネル✓ + メタ）。罫線・影なし、地の上にメディアとテキストが乗るだけ。サムネ右下に時間バッジ、左下に LIVE バッジ。
- **状態**: ホバーは `--s-bg-hover` に一段持ち上げ。選択ナビ行は `--s-bg-selected`。フィルタチップの選択は明色反転（`--s-bg-inverse` / `--s-text-inverse`）。
- **レイアウト**: 一覧は `--s-grid-columns`（既定 4）カラム、間隔 `--s-space-gap-loose`。サムネは常に 16:9。

## アイコン
- 単色のラインアイコン（24px 標準、`fill="currentColor"` で文字色を継承）。色は `--s-icon-default`(#CCCCCC) / `--s-icon-muted`(#717171, ケバブ等) / `--s-icon-verified`(#AAAAAA)。
- `assets/icons/` に汎用 SVG を同梱（ホーム/登録チャンネル/履歴/再生リスト系、ケバブ、検索、再生、認証チェック等）。アイコンフォントは使わない。

## 索引（マニフェスト）
- `styles.css` — エントリ（fonts → primitives → semantic を @import）。
- `tokens/` — `fonts.css` / `primitives.css` / `semantic.css`。
- `components/` — `media/`（VideoCard, RowItem, PlaylistCard, Avatar）, `badges/`（LiveBadge, DurationBadge）, `controls/`（PillButton, FilterChip）, `navigation/`（NavItem, ChannelRow）。各 `.jsx` + `.d.ts` + `.prompt.md` + デモカード。
- `ui_kits/youtube-super-lite/` — `index.html`（おすすめグリッド）, `history.html`（再生履歴）, `playlists.html`（再生リスト）, README。
- `guidelines/` — 基礎カード（Colors / Type / Spacing / Brand）。
- `assets/icons/` — 汎用アイコン SVG。

## 注意事項
- **フォントは Google Fonts**（Roboto / Noto Sans JP）を CDN 読込。バイナリ同梱なし。ライセンス上必要なら差し替え可。
- **デモカードは静的な（トークン駆動の）HTML**。生成バンドルの名前空間に依存せず確実に表示される。`.jsx`/`.d.ts` は引き続き利用側・Starting Points 用にバンドルされる。
- **サムネイルはプレースホルダ**。
- 色値はスクリーンショットからの実測・推定に由来する。実装時に実物と突き合わせて微調整する前提。
