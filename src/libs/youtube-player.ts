declare global {
  interface Window {
    onYouTubeIframeAPIReady: () => void;
    YT: typeof YT | undefined;
  }
}

type InitPlayer = (params: { domId: string } & YT.PlayerOptions) => Promise<{
  player: YT.Player;
  destroy: () => void;
}>;
export const initPlayer: InitPlayer = ({ domId, ...options }) => {
  if (window.YT?.Player) {
    const player = new window.YT.Player(domId, options);
    return Promise.resolve({
      player,
      destroy: () => player?.destroy(),
    });
  }

  const tag = document.createElement("script");
  tag.src = "https://www.youtube.com/iframe_api";
  document.body.appendChild(tag);

  return new Promise<{
    player: YT.Player;
    destroy: () => void;
  }>((resolve) => {
    window.onYouTubeIframeAPIReady = () => {
      const player = new YT.Player(domId, options);
      resolve({
        player,
        destroy: () => player?.destroy(),
      });
    };
  });
};
