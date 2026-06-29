import * as React from 'react';
/** Playlist card with stacked-thumbnail treatment and a count overlay badge. */
export interface PlaylistCardProps {
  thumb?: string; thumbBg?: string; count: number | string;
  title: string; meta?: string; updated?: string; linkLabel?: string;
}
export function PlaylistCard(props: PlaylistCardProps): JSX.Element;
