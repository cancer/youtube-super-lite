import {
  createSignal,
  onCleanup,
  onMount,
  Show,
  type VoidComponent,
} from "solid-js";
import { initPlayer, loadPlayer } from "~/libs/youtube-player";
import { LikeButton } from "~/routes/watch/like-button";

type Props = {
  videoId: string;
  rating: string | null;
  onClickLike: () => void;
};
export const Player: VoidComponent<Props> = (props) => {
  // YouTubeプレーヤー自体の読み込み
  onMount(() => loadPlayer());

  // 読み込まれたプレーヤーを使って動画再生
  let player: YT.Player;
  let containerEl: HTMLDivElement;
  let playerEl: HTMLDivElement;
  onMount(async () => {
    const { width } = containerEl.getBoundingClientRect();
    player = await initPlayer(playerEl, {
      width,
      height: width * 0.5625, // 16:9
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
      "resize",
      () => {
        const { width } = containerEl.getBoundingClientRect();
        player.setSize(width, width * 0.5625);
      },
      true,
    );
  });
  onMount(() => {
    window.addEventListener(
      "keypress",
      (ev) => {
        if (ev.key !== " ") return;
        ev.preventDefault();

        if (player.getPlayerState() === YT.PlayerState.PLAYING)
          player.pauseVideo();
        else player.playVideo();
      },
      true,
    );
  });

  onCleanup(() => player.destroy?.());

  return (
    <div class="grid grid-cols-2 gap-2 w-full">
      <div ref={containerEl!} class="col-span-full grid-row-1">
        <div ref={playerEl!} />
      </div>
      <div class="grid-row-2">
        <Show when={props.rating}>
          {(data) => (
            <LikeButton liked={data() === "like"} onClick={props.onClickLike} />
          )}
        </Show>
      </div>
      <div class="grid-row-2 flex justify-end items-start gap-2 pt-2">
        <button onClick={() => player.setPlaybackRate(1)}>x1.0</button>
        <button onClick={() => player.setPlaybackRate(1.5)}>x1.5</button>
        <button onClick={() => player.setPlaybackRate(2)}>x2.0</button>
      </div>
    </div>
  );
};
