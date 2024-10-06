declare global {
  interface Window {
    onYouTubeIframeAPIReady: () => void;
    YT: typeof YT | undefined;
    addEventListener(
      type: "youTubeIframeAPIReady",
      listener: (
        this: Window,
        event: CustomEvent<{ Player: typeof YT.Player }>,
      ) => void,
      options?: AddEventListenerOptions,
    ): void;
  }
}

const createLoadEvent = (Player: typeof YT.Player) =>
  new CustomEvent("youTubeIframeAPIReady", {
    detail: { Player },
  });

type LoadPlayer = () => void;
export const loadPlayer: LoadPlayer = () => {
  if (window.YT?.Player) {
    window.dispatchEvent(createLoadEvent(window.YT.Player));
    return;
  }

  const tag = document.createElement("script");
  tag.src = "https://www.youtube.com/iframe_api";
  document.body.appendChild(tag);

  window.onYouTubeIframeAPIReady = () => {
    window.dispatchEvent(createLoadEvent(window.YT!.Player));
  };
};

type InitPlayer = (
  container: HTMLElement,
  params: YT.PlayerOptions,
) => Promise<YT.Player>;
export const initPlayer: InitPlayer = (container, options) => {
  if (window.YT?.Player) {
    const player = new window.YT.Player(container, options);
    return Promise.resolve(player);
  }

  const tag = document.createElement("script");
  tag.src = "https://www.youtube.com/iframe_api";
  document.body.appendChild(tag);

  return new Promise((resolve) => {
    window.addEventListener(
      "youTubeIframeAPIReady",
      ({ detail: { Player } }) => {
        const player = new Player(container, options);
        resolve(player);
      },
      { once: true },
    );
  });
};
