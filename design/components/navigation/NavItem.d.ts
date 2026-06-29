/** Sidebar nav row with inline-SVG icon and selected state. */
export interface NavItemProps { icon: string; label: string; selected?: boolean; onClick?: () => void; }
export function NavItem(props: NavItemProps): JSX.Element;
