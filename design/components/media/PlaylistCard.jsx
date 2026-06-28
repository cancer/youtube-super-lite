import React from 'react';
import { DurationBadge } from '../badges/DurationBadge.jsx';

/** Playlist card (DESIGN.md §4.8): stacked thumbnail + count overlay + title/meta/link. */
export function PlaylistCard({ thumb, thumbBg, count, title, meta = '非公開・プレイリスト', updated = '本日更新', linkLabel = '再生リストの全体を見る' }) {
  const listIcon = '<svg viewBox="0 0 24 24" fill="currentColor" width="13" height="13"><path d="M3 10h11v2H3zm0-4h11v2H3zm0 8h7v2H3zm13-2v6l5-3z"/></svg>';
  return (
    <div style={{ minWidth: 0, cursor: 'pointer' }}>
      <div style={{ position: 'relative' }}>
        <div style={{ position: 'absolute', left: '6%', right: '6%', top: -6, height: 10, borderRadius: 'var(--s-radius-container)', background: 'var(--s-bg-elevated)' }} />
        <div style={{ position: 'absolute', left: '3%', right: '3%', top: -3, height: 10, borderRadius: 'var(--s-radius-container)', background: 'var(--s-bg-hover)' }} />
        <div style={{ position: 'relative', width: '100%', aspectRatio: 'var(--s-ratio-media)', borderRadius: 'var(--s-radius-container)',
          overflow: 'hidden', background: thumb ? `center/cover no-repeat url(${thumb})` : (thumbBg || 'var(--s-bg-surface)') }}>
          <DurationBadge icon={listIcon} style={{ position: 'absolute', right: 'var(--s-space-overlay-offset)', bottom: 'var(--s-space-overlay-offset)' }}>{count} 本の動画</DurationBadge>
        </div>
      </div>
      <div style={{ paddingTop: 'var(--s-space-gap-tight)' }}>
        <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-md)', lineHeight: 'var(--p-lh-md)', fontWeight: 'var(--p-weight-medium)', color: 'var(--s-text-primary)' }}>{title}</div>
        <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-sm)', color: 'var(--s-text-secondary)', marginTop: 4 }}>{meta}</div>
        <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-sm)', color: 'var(--s-text-secondary)', marginTop: 2 }}>{updated}</div>
        <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-sm)', color: 'var(--s-text-secondary)', marginTop: 6 }}>{linkLabel}</div>
      </div>
    </div>
  );
}
