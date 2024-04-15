import { onCleanup, onMount, type VoidComponent } from "solid-js";
import { initPlayer } from "~/libs/youtube-player";

type Props = {
  videoId: string;
};
export const Player: VoidComponent<Props> = (props) => {
  let container: HTMLDivElement;
  let destroy: () => void;
  onMount(async () => {
    const { width } = container.getBoundingClientRect();
    const { player, destroy: _destroy } = await initPlayer({
      domId: "player",
      width,
      height: width * 0.5625, // 16:9
      events: {
        onReady: ({ target }) => {
          target.loadVideoById(props.videoId, 0, "hd1080");
        },
      },
    });
    destroy = _destroy;

    window.addEventListener(
      "resize",
      () => {
        const { width } = container.getBoundingClientRect();
        player.setSize(width, width * 0.5625);
      },
      true,
    );
  });
  onCleanup(() => destroy?.());

  return (
    <div ref={container!}>
      <div id="player" />
    </div>
  );
};
