import { onMount, type VoidComponent } from "solid-js";
import { isServer } from "solid-js/web";
import { initPlayer } from "~/libs/youtube-player";

type Props = {
  videoId: string;
};
export const Player: VoidComponent<Props> = (props) => {
  onMount(async () => {
    if (isServer) return;
    await initPlayer({
      domId: "player",
      width: 1280,
      height: 720,
      videoId: props.videoId,
      events: {
        onReady: (event) => {
          //event.target.playVideo();
        },
      },
    });
  });

  return <div id="player" />;
};
