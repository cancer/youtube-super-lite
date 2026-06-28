/** Sidebar channel row: nav avatar + name + optional live/notify dot. */
export interface ChannelRowProps { name: string; initial?: string; src?: string; color?: string; dot?: 'live' | 'notify'; onClick?: () => void; }
export function ChannelRow(props: ChannelRowProps): JSX.Element;
