type ParseYouTubeUrl = (
  urlStr: string,
) => { type: "video"; id: string } | { type: "unknown" };
export const parseYouTubeUrl: ParseYouTubeUrl = (urlStr) => {
  const url = new URL(urlStr);

  if (url.pathname === "/watch") {
    return { type: "video", id: url.searchParams.get("v")! };
  }

  return { type: "unknown" };
};
