import React from 'react';

/** Circular channel avatar. `role` picks the standard size (channel 36 / nav 24).
 * Optional status `dot`: 'live' (brand red) or 'notify'. */
export function Avatar({ src, initial = '', color = 'var(--s-bg-elevated)', role = 'channel', dot, style }) {
  const size = role === 'nav' ? 'var(--s-size-avatar-nav)' : 'var(--s-size-avatar-channel)';
  const px = role === 'nav' ? 24 : 36;
  const dotColor = dot === 'notify' ? 'var(--s-indicator-notify)' : 'var(--s-accent-brand)';
  return (
    <span style={{ position: 'relative', display: 'inline-flex', width: size, height: size, flex: `0 0 ${px}px`, ...style }}>
      <span style={{ width: size, height: size, borderRadius: 'var(--s-radius-circle)', overflow: 'hidden',
        background: src ? `center/cover no-repeat url(${src})` : color, display: 'flex', alignItems: 'center',
        justifyContent: 'center', color: 'var(--s-text-primary)', fontFamily: 'var(--p-font-sans)',
        fontWeight: 'var(--p-weight-bold)', fontSize: px * 0.42 }}>{!src && initial}</span>
      {dot && <span style={{ position: 'absolute', bottom: -1, right: -1, width: 'var(--s-size-indicator)',
        height: 'var(--s-size-indicator)', borderRadius: 'var(--s-radius-circle)', background: dotColor,
        border: '2px solid var(--s-bg-canvas)' }} />}
    </span>
  );
}
