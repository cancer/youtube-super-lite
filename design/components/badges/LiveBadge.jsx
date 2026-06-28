import React from 'react';

/** LIVE badge — bottom-left of a thumbnail. Solid red, white label, overlay radius. */
export function LiveBadge({ label = 'ライブ', dot = true, style }) {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, background: 'var(--s-accent-live)',
      color: 'var(--s-text-on-accent)', fontFamily: 'var(--p-font-sans)', fontWeight: 'var(--p-weight-medium)',
      fontSize: 'var(--p-size-sm)', lineHeight: 1, padding: '3px 6px', borderRadius: 'var(--s-radius-overlay)', ...style }}>
      {dot && <span style={{ width: 6, height: 6, borderRadius: '50%', background: 'var(--s-text-on-accent)' }} />}
      {label}
    </span>
  );
}
