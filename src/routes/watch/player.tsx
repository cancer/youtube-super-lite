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
  const [player, setPlayer] = createSignal<YT.Player | null>(null);

  // YouTubeプレーヤー自体の読み込み
  onMount(() => loadPlayer());

  // 読み込まれたプレーヤーを使って動画再生
  let containerEl: HTMLDivElement;
  let playerEl: HTMLDivElement;
  onMount(async () => {
    const { width } = containerEl.getBoundingClientRect();
    setPlayer(
      await initPlayer(playerEl, {
        width,
        height: width * 0.5625, // 16:9
        events: {
          onReady: ({ target }) => {
            target.loadVideoById(props.videoId, 0, "hd1080");
          },
        },
      }),
    );
  });
  onCleanup(() => player()?.destroy?.());

  return (
    <div class="grid grid-cols-2 grid-rows-2 gap-2 w-full">
      <div ref={containerEl!} class="col-span-full grid-row-1">
        <div ref={playerEl!} />
      </div>
      <Show when={props.rating}>
        {(data) => (
          <div class="grid-row-2">
            <LikeButton liked={data() === "like"} onClick={props.onClickLike} />
          </div>
        )}
      </Show>
      <Show when={player()}>
        {(_player) => (
          <div class="grid-row-2 flex justify-end items-start gap-2 pt-2">
            <button onClick={() => _player().setPlaybackRate(1)}>x1.0</button>
            <button onClick={() => _player().setPlaybackRate(1.5)}>x1.5</button>
            <button onClick={() => _player().setPlaybackRate(2)}>x2.0</button>
          </div>
        )}
      </Show>
    </div>
  );
};
