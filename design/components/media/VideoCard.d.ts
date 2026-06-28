import * as React from 'react';
/**
 * Video grid card: 16:9 thumbnail (with duration or LIVE badge) + channel avatar,
 * 2-line title, channel name, and "views・age" meta. The atomic grid unit.
 * @startingPoint section="Media" subtitle="Video grid card" viewport="320x300"
 */
export interface VideoCardProps {
  thumb?: string;
  thumbBg?: string;
  duration?: string;
  live?: boolean;
  title: string;
  channel: string;
  verified?: boolean;
  views?: string;
  age?: string;
  avatarSrc?: string;
  avatarColor?: string;
  initial?: string;
}
export function VideoCard(props: VideoCardProps): JSX.Element;
