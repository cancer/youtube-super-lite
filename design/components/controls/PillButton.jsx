import React from 'react';

/** Pill button (DESIGN.md §4.3) — e.g. もっと見る. Elevated bg, optional trailing caret. */
export function PillButton({ children, caret = false, onClick, style }) {
  const [hover, setHover] = React.useState(false);
  return (
    <button onClick={onClick} onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      style={{ display: 'inline-flex', alignItems: 'center', gap: 6, border: 'none', cursor: 'pointer',
        fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-md)', fontWeight: 'var(--p-weight-medium)',
        color: 'var(--s-text-primary)', background: hover ? 'var(--s-bg-hover)' : 'var(--s-bg-elevated)',
        padding: 'var(--s-space-overlay-offset) var(--s-space-inset-pill)', borderRadius: 'var(--s-radius-control-pill)', ...style }}>
      {children}
      {caret && <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M7 10l5 5 5-5z"/></svg>}
    </button>
  );
}
