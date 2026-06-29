import React from 'react';

/** Sidebar nav row (DESIGN.md §4.5). `icon` is an inline SVG string (uses currentColor). */
export function NavItem({ icon, label, selected = false, onClick }) {
  const [hover, setHover] = React.useState(false);
  const bg = selected ? 'var(--s-bg-selected)' : (hover ? 'var(--s-bg-hover)' : 'transparent');
  return (
    <div onClick={onClick} onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      style={{ display: 'flex', alignItems: 'center', gap: 'var(--s-space-gap-tight)', height: 'var(--s-size-nav-row)',
        padding: '0 var(--s-space-inset)', borderRadius: 'var(--s-radius-control-soft)', cursor: 'pointer', background: bg,
        color: 'var(--s-text-primary)', fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-md)',
        fontWeight: selected ? 'var(--p-weight-medium)' : 'var(--p-weight-regular)' }}>
      <span style={{ width: 'var(--s-size-icon)', height: 'var(--s-size-icon)', display: 'flex', color: 'var(--s-icon-default)' }} dangerouslySetInnerHTML={{ __html: icon }} />
      <span>{label}</span>
    </div>
  );
}
