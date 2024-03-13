type InitPlayer = (
  params: { domId: string } & YT.PlayerOptions,
) => Promise<YT.Player>;
export const initPlayer: InitPlayer = (params) => {
  const tag = document.createElement("script");
  tag.src = "https://www.youtube.com/iframe_api";
  document.body.appendChild(tag);

  const { domId, ...options } = params;
  return new Promise((resolve) => {
    (window as unknown as any).onYouTubeIframeAPIReady = () => {
      resolve(new YT.Player(domId, options));
    };
  });
};
