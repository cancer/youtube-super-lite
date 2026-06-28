import * as React from 'react';
/** Circular channel avatar with initial fallback and optional live/notify status dot. */
export interface AvatarProps {
  src?: string;
  initial?: string;
  color?: string;
  role?: 'channel' | 'nav';
  dot?: 'live' | 'notify';
  style?: React.CSSProperties;
}
export function Avatar(props: AvatarProps): JSX.Element;
