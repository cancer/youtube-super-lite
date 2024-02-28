import { createEffect, onMount, type VoidComponent } from "solid-js";
import { isServer } from "solid-js/web";
import {
  initPlayer,
  type YouTubePlayer,
} from "~/libs/youtube-player";

type Props = {
  videoId: string;
};
export const Player: VoidComponent<Props> = (props) => {
  let player: YouTubePlayer | null = null;
  onMount(async () => {
    if (isServer) return;
    player = await initPlayer({
      domId: "player",
      width: 1280,
      height: 720,
      videoId: props.videoId,
    });
  });

  createEffect(() => {
    // 触っとかないとreactiveにならない
    const videoId = props.videoId;
    if (player === null) return;
    player.loadVideoById(videoId, 0, "hd1080");
  });

  return <div id="player" />;
};
