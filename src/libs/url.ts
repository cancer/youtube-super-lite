type VideoNavigation = { type: "video"; id: string };
type UnknownNavigation = { type: "unknown" };
export type YTNavigation = VideoNavigation | UnknownNavigation;

// TODO: https://youtu.be/2wczkeeoYQc にも対応できるように
type ParseYouTubeUrl = (
  urlStr: string,
) => YTNavigation;
export const parseYouTubeUrl: ParseYouTubeUrl = (urlStr) => {
  const url = new URL(urlStr);

  if (url.pathname === "/watch") {
    return { type: "video", id: url.searchParams.get("v")! };
  }

  return { type: "unknown" };
};
