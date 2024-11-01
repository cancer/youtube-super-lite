import { type JSX, onCleanup, onMount, type VoidComponent } from "solid-js";
import { initPlayer, loadPlayer } from "~/libs/youtube-player";

import "./player.css";

type Props = {
  videoId: string;
  onClickClose: () => void;
  LikeButton: JSX.Element;
};
export const Player: VoidComponent<Props> = (props) => {
  // YouTubeプレーヤー自体の読み込み
  onMount(() => loadPlayer());

  // 読み込まれたプレーヤーを使って動画再生
  let player: YT.Player;
  let playerEl: HTMLDivElement;
  onMount(async () => {
    player = await initPlayer(playerEl, {
      width: "auto",
      height: "auto",
      events: {
        onReady: ({ target }) => {
          target.loadVideoById(props.videoId, 0, "hd1080");
        },
      },
    });
  });

  // イベントハンドリング
  onMount(() => {
    window.addEventListener(
      "keypress",
      (ev) => {
        if (ev.key !== " ") return;
        if (!player) return;
        ev.preventDefault();

        if (player.getPlayerState() === YT.PlayerState.PLAYING)
          player.pauseVideo();
        else player.playVideo();
      },
      true,
    );
  });

  onCleanup(() => player?.destroy());

  return (
    <div class="playerComponent group grid grid-cols-2 grid-rows-[1fr_max-content] gap-2 w-max h-full relative">
      <div class="absolute w-full h-full scale-0 group-hover:scale-100 pointer-events-none">
        <button class="pointer-events-auto" onClick={props.onClickClose}>
          とじる
        </button>
      </div>
      <div class="w-max col-span-full grid-row-1">
        <div ref={playerEl!} />
      </div>
      <div class="grid-row-2">{props.LikeButton}</div>
      <div class="w-full grid-row-2">
        <div class="flex justify-end items-start gap-2 pt-2">
          <button onClick={() => player.setPlaybackRate(1)}>x1.0</button>
          <button onClick={() => player.setPlaybackRate(1.5)}>x1.5</button>
          <button onClick={() => player.setPlaybackRate(2)}>x2.0</button>
        </div>
      </div>
    </div>
  );
};
