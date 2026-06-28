import * as React from 'react';
/** Translucent scrim badge for duration ("1:11:13") or a playlist count ("100 本の動画"). */
export interface DurationBadgeProps { children: React.ReactNode; icon?: string; style?: React.CSSProperties; }
export function DurationBadge(props: DurationBadgeProps): JSX.Element;
