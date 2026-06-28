import React from 'react';

/** Duration / count overlay badge — bottom-right of a thumbnail. Scrim bg, white badge text. */
export function DurationBadge({ children, icon, style }) {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, background: 'var(--s-bg-scrim)',
      color: 'var(--s-text-on-accent)', fontFamily: 'var(--p-font-sans)', fontWeight: 'var(--p-weight-medium)',
      fontSize: 'var(--p-size-sm)', lineHeight: 1, padding: '3px 5px', borderRadius: 'var(--s-radius-overlay)', ...style }}>
      {icon && <span style={{ display: 'flex', width: 13, height: 13 }} dangerouslySetInnerHTML={{ __html: icon }} />}
      {children}
    </span>
  );
}
