import React from 'react';
import { Avatar } from '../media/Avatar.jsx';

/** Channel row inside the sidebar (DESIGN.md §4.6): small avatar + name + status dot. */
export function ChannelRow({ name, initial, src, color, dot, onClick }) {
  const [hover, setHover] = React.useState(false);
  return (
    <div onClick={onClick} onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      style={{ display: 'flex', alignItems: 'center', gap: 'var(--s-space-gap-tight)', height: 'var(--s-size-nav-row)',
        padding: '0 var(--s-space-inset)', borderRadius: 'var(--s-radius-control-soft)', cursor: 'pointer',
        background: hover ? 'var(--s-bg-hover)' : 'transparent' }}>
      <Avatar role="nav" src={src} initial={initial} color={color || 'var(--s-bg-elevated)'} dot={dot} />
      <span style={{ fontFamily: 'var(--p-font-sans)', fontSize: 'var(--p-size-md)', color: 'var(--s-text-primary)',
        whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</span>
    </div>
  );
}
