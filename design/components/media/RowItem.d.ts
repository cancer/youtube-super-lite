import * as React from 'react';
/** Horizontal history row: large left thumbnail + title/channel/meta column + kebab. */
export interface RowItemProps {
  thumb?: string; thumbBg?: string; duration?: string;
  title: string; channel: string; views?: string; age?: string; thumbWidth?: number;
}
export function RowItem(props: RowItemProps): JSX.Element;
