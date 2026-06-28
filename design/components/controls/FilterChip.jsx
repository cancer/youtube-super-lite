import React from 'react';

/** Filter chip (DESIGN.md §4.4). Selected state inverts to a light surface. */
export function FilterChip({ children, selected = false, onClick, style }) {
  const [hover, setHover] = React.useState(false);
  const bg = selected ? 'var(--s-bg-inverse)' : (hover ? 'var(--s-bg-hover)' : 'var(--s-bg-elevated)');
  const color = selected ? 'var(--s-text-inverse)' : 'var(--s-text-primary)';
  return (
    <button onClick={onClick} onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      style={{ border: 'none', cursor: 'pointer', fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-md)',
        fontWeight: 'var(--p-weight-medium)', background: bg, color, padding: '6px var(--s-space-inset)',
        borderRadius: 'var(--s-radius-control-soft)', whiteSpace: 'nowrap', ...style }}>
      {children}
    </button>
  );
}
