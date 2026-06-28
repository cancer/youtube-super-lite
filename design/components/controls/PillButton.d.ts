import * as React from 'react';
/**
 * Fully-rounded pill button (もっと見る, sort dropdown). `caret` adds a trailing chevron.
 * @startingPoint section="Controls" subtitle="Pill button" viewport="700x120"
 */
export interface PillButtonProps { children: React.ReactNode; caret?: boolean; onClick?: () => void; style?: React.CSSProperties; }
export function PillButton(props: PillButtonProps): JSX.Element;
