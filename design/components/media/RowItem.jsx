import React from 'react';
import { DurationBadge } from '../badges/DurationBadge.jsx';

/** Horizontal list row (DESIGN.md §4.7) — used in 再生履歴. Large thumb + text column + kebab. */
export function RowItem({ thumb, thumbBg, duration, title, channel, views, age, thumbWidth = 240 }) {
  const [hover, setHover] = React.useState(false);
  return (
    <div style={{ display: 'flex', gap: 'var(--s-space-gap-loose)', cursor: 'pointer' }}>
      <div style={{ position: 'relative', width: thumbWidth, flex: `0 0 ${thumbWidth}px`, aspectRatio: 'var(--s-ratio-media)',
        borderRadius: 'var(--s-radius-container)', overflow: 'hidden', background: thumb ? `center/cover no-repeat url(${thumb})` : (thumbBg || 'var(--s-bg-surface)') }}>
        {duration && <DurationBadge style={{ position: 'absolute', right: 'var(--s-space-overlay-offset)', bottom: 'var(--s-space-overlay-offset)' }}>{duration}</DurationBadge>}
      </div>
      <div style={{ flex: 1, minWidth: 0, paddingTop: 2 }}>
        <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-lg)', lineHeight: 'var(--p-lh-lg)',
          fontWeight: 'var(--p-weight-medium)', color: 'var(--s-text-primary)', display: '-webkit-box',
          WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>{title}</div>
        <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-sm)', color: 'var(--s-text-secondary)', marginTop: 6 }}>{channel}</div>
        <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-sm)', color: 'var(--s-text-secondary)', marginTop: 2 }}>{[views, age].filter(Boolean).join('・')}</div>
      </div>
      <span onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
        style={{ flex: '0 0 32px', width: 32, height: 32, display: 'flex', alignItems: 'center', justifyContent: 'center',
          borderRadius: 'var(--s-radius-circle)', color: 'var(--s-icon-muted)', background: hover ? 'var(--s-bg-hover)' : 'transparent' }}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><path d="M12 8c1.1 0 2-.9 2-2s-.9-2-2-2-2 .9-2 2 .9 2 2 2zm0 2c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm0 6c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2z"/></svg>
      </span>
    </div>
  );
}
