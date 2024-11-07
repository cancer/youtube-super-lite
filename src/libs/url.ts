type VideoNavigation = { type: "video"; id: string };
type UnknownNavigation = { type: "unknown" };
export type YTNavigation = VideoNavigation | UnknownNavigation;

type ParseYouTubeUrl = (urlStr: string) => YTNavigation;
export const parseYouTubeUrl: ParseYouTubeUrl = (urlStr) => {
  const url = new URL(urlStr);

  if (url.hostname === "youtu.be")
    return { type: "video", id: url.pathname.replace("/", "") };

  if (url.pathname === "/watch")
    return { type: "video", id: url.searchParams.get("v")! };

  return { type: "unknown" };
};
