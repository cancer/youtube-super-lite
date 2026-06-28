import * as React from 'react';
/** Single-select filter chip; `selected` inverts fg/bg. */
export interface FilterChipProps { children: React.ReactNode; selected?: boolean; onClick?: () => void; style?: React.CSSProperties; }
export function FilterChip(props: FilterChipProps): JSX.Element;
