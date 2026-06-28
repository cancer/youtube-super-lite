import React from 'react';
import { Avatar } from './Avatar.jsx';
import { LiveBadge } from '../badges/LiveBadge.jsx';
import { DurationBadge } from '../badges/DurationBadge.jsx';

/** The grid's core unit (DESIGN.md §4.1): 16:9 thumbnail + duration/LIVE badges,
 * then channel avatar + 2-line title + channel + meta + kebab. */
export function VideoCard({ thumb, thumbBg, duration, live = false, title, channel, verified = true,
  views, age, avatarSrc, avatarColor, initial = '' }) {
  const [hover, setHover] = React.useState(false);
  return (
    <div style={{ minWidth: 0, cursor: 'pointer' }}>
      <div style={{ position: 'relative', width: '100%', aspectRatio: 'var(--s-ratio-media)',
        borderRadius: 'var(--s-radius-container)', overflow: 'hidden',
        background: thumb ? `center/cover no-repeat url(${thumb})` : (thumbBg || 'var(--s-bg-surface)') }}>
        {live && <LiveBadge style={{ position: 'absolute', left: 'var(--s-space-overlay-offset)', bottom: 'var(--s-space-overlay-offset)' }} />}
        {duration && !live && <DurationBadge style={{ position: 'absolute', right: 'var(--s-space-overlay-offset)', bottom: 'var(--s-space-overlay-offset)' }}>{duration}</DurationBadge>}
      </div>
      <div style={{ display: 'flex', gap: 'var(--s-space-gap-tight)', paddingTop: 'var(--s-space-gap-tight)' }}>
        <Avatar src={avatarSrc} initial={initial} color={avatarColor || 'var(--s-bg-elevated)'} role="channel" />
        <div style={{ minWidth: 0, flex: 1 }}>
          <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-md)', lineHeight: 'var(--p-lh-md)',
            fontWeight: 'var(--p-weight-medium)', color: 'var(--s-text-primary)', display: '-webkit-box',
            WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>{title}</div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginTop: 4, fontFamily: 'var(--p-font-sans)',
            fontSize: 'var(--p-size-sm)', color: 'var(--s-text-secondary)' }}>
            <span style={{ whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{channel}</span>
            {verified && <span style={{ color: 'var(--s-icon-verified)', flex: '0 0 auto' }}>&#10003;</span>}
          </div>
          <div style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-sm)', color: 'var(--s-text-secondary)' }}>
            {[live ? null : views, age].filter(Boolean).join('・')}{live && views ? views : ''}
          </div>
        </div>
        <span onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
          style={{ flex: '0 0 24px', width: 24, height: 24, display: 'flex', alignItems: 'center', justifyContent: 'center',
            borderRadius: 'var(--s-radius-circle)', color: 'var(--s-icon-muted)', background: hover ? 'var(--s-bg-hover)' : 'transparent' }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M12 8c1.1 0 2-.9 2-2s-.9-2-2-2-2 .9-2 2 .9 2 2 2zm0 2c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm0 6c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2z"/></svg>
        </span>
      </div>
    </div>
  );
}
