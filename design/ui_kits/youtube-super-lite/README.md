# YouTube Super Lite — UI kit

High-fidelity recreations of the three list surfaces from **YouTube Super Lite** (Rust/Windows native player). These are the views that appear as the full-screen list overlay (Tab) over the mpv video layer.

## Screens
- **index.html** — おすすめ 動画グリッド: sidebar + filter chips + 4-column `VideoCard` grid + もっと見る pill.
- **history.html** — 再生履歴: page title + filter chips + date `SectionHeading`s + `RowItem` rows.
- **playlists.html** — 再生リスト: page title + sort `PillButton` + stacked `PlaylistCard` grid.

## Notes
- Composed from the design-system primitives (VideoCard, RowItem, PlaylistCard, NavItem, ChannelRow, FilterChip, PillButton, badges).
- All values come from semantic tokens (`--s-*`); grids use `repeat(var(--s-grid-columns),1fr)` with `min-width:0` items.
- Thumbnails are neutral placeholders; the real app decodes thumbnails via WIC and caches them to disk.
- In the live product these lists render in a translucent Direct2D overlay over the video; here they are shown opaque on `--s-bg-canvas` to document the layout/tokens.
