import {
  createSignal,
  onCleanup,
  onMount,
  Show,
  type VoidComponent,
} from "solid-js";
import { initPlayer } from "~/libs/youtube-player";
import { LikeButton } from "~/routes/watch/like-button";

type Props = {
  videoId: string;
  rating: string | null;
  onClickLike: () => void;
};
export const Player: VoidComponent<Props> = (props) => {
  const [player, setPlayer] = createSignal<YT.Player | null>(null);
  let container: HTMLDivElement;
  let destroy: () => void;
  onMount(async () => {
    const { width } = container.getBoundingClientRect();
    const { player: _player, destroy: _destroy } = await initPlayer({
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
    setPlayer(_player);

    window.addEventListener(
      "resize",
      () => {
        const { width } = container.getBoundingClientRect();
        _player.setSize(width, width * 0.5625);
        setPlayer(_player);
      },
      true,
    );
  });
  onCleanup(() => destroy?.());

  return (
    <div class="grid grid-cols-2 grid-rows-2">
      <div ref={container!} class="col-span-full grid-row-1">
        <div id="player" />
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
          <div class="grid-row-2 flex justify-end items-start gap-2">
            <button onClick={() => _player().setPlaybackRate(1)}>x1.0</button>
            <button onClick={() => _player().setPlaybackRate(1.5)}>x1.5</button>
            <button onClick={() => _player().setPlaybackRate(2)}>x2.0</button>
          </div>
        )}
      </Show>
    </div>
  );
};
