import {
  action,
  cache,
  createAsync,
  type RouteDefinition,
  useAction,
  useSearchParams,
} from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import {
  getVideoRating,
  postVideoRating,
  type VideoRatingResponse,
} from "~/libs/api/youtube";
import { parseYouTubeUrl } from "~/libs/url";
import { Header } from "~/uis/header";
import { getLoginStatus, LoginButton, LogoutButton } from "~/uis/login-button";
import { WatchVideoFromYouTube } from "~/uis/watch-video-from-you-tube";

const Player = clientOnly(() =>
  import("./player").then(({ Player }) => ({ default: Player })),
);

const fetchRatings = cache(async (params: { ids: string[] }) => {
  "use server";
  const { youtubeApi } = getRequestEvent()!.locals;
  let ratings: Map<string, VideoRatingResponse["GET"]>;
  try {
    ratings = new Map(
      await Promise.all(
        params.ids.map((id) =>
          getVideoRating(youtubeApi)({ id }).then((res) => [id, res] as const),
        ),
      ),
    );
  } catch {
    return null;
  }
  return ratings;
}, "ratings");

const likeAction = action(async (id: string) => {
  "use server";
  const { youtubeApi } = getRequestEvent()!.locals;
  await postVideoRating(youtubeApi)({ id, rating: "like" });
  return null;
});

type Params = { videoIds: string };

export const routes = {
  load: async () => {
    const { youtubeApi } = getRequestEvent()!.locals;
    const [{ videoIds }] = useSearchParams<Params>();
    if (!videoIds) return null;

    const ratings = await Promise.all(
      videoIds
        .split(",")
        .map((id) =>
          getVideoRating(youtubeApi)({ id }).then((res) => [id, res] as const),
        ),
    ).catch(() => {
      // https://github.com/solidjs/solid-router/issues/399
      return null;
    });
    if (!ratings) return null;

    return new Map(ratings);
  },
} satisfies RouteDefinition;

const Watch = () => {
  const [searchParams, setSearchParams] = useSearchParams<Params>();

  const [videoIds, setVideoIds] = createSignal<string[]>(
    searchParams.videoIds?.split(",") ?? [],
  );
  const [liked, setLiked] = createSignal(false);
  const isLoggedIn = createAsync(() => getLoginStatus());
  const ratings = createAsync(async () => fetchRatings({ ids: videoIds() }), {
    deferStream: true,
  });

  const divisions = createMemo(() => Math.ceil(Math.sqrt(videoIds().length)));

  createEffect(() => {
    if (videoIds().length === 0) return;
    // 変化がないのにsearchParamsを更新すると、`/`にリダイレクトされてしまう
    if (videoIds().join(",") === searchParams.videoIds) return;
    setSearchParams({ videoIds: videoIds().join(",") });
  });

  const like = useAction(likeAction);

  // Tailwind的なCSSは、動的にclass文字列を作って使うことができないらしい
  // e.g.)
  //   NG class={`grid grid-cols-${divisions()}`}
  //   OK class={`grid ${gridCols.get(divisions())}`}
  const squareDivisionsMap = new Map([
    [1, "grid-cols-1 grid-rows-1"],
    [2, "grid-cols-2 grid-rows-2"],
    [3, "grid-cols-3 grid-rows-3"],
    [4, "grid-cols-4 grid-rows-4"],
  ]);

  return (
    <div class="w-screen h-screen grid grid-rows-[max-content_1fr] justify-center">
      <div class="w-screen h-max col-span-full">
        <Header
          LeftSide={
            <Show when={videoIds().length > 0}>
              <WatchVideoFromYouTube
                onSubmit={(ev) => {
                  ev.preventDefault();

                  if (ev.currentTarget.url.value === "") return;

                  const parsed = parseYouTubeUrl(ev.currentTarget.url.value);
                  if (parsed.type !== "video") return;

                  if (
                    (ev.submitter as HTMLButtonElement).name === "openNewPage"
                  ) {
                    setVideoIds([parsed.id]);
                    ev.currentTarget.url.value = "";
                    return;
                  }

                  if (videoIds().length === 16)
                    return console.warn("Maximum number of videos reached.");

                  setVideoIds((prev) => [...prev, parsed.id]);
                  ev.currentTarget.url.value = "";
                }}
                Action={
                  <>
                    <button type="submit" name="openCurrentPage">
                      👇 Add
                    </button>
                    <button type="submit" name="openNewPage">
                      👉 Go
                    </button>
                  </>
                }
              ></WatchVideoFromYouTube>
            </Show>
          }
          RightSide={
            <Show when={isLoggedIn()} fallback={<LoginButton />}>
              <LogoutButton />
            </Show>
          }
        />
      </div>
      <div class="grid justify-center">
        <div class="grid justify-center items-center h-full">
          <Show
            when={videoIds().length > 0 && videoIds()}
            fallback={
              <div class="grid justify-center items-center w-max h-max">
                <WatchVideoFromYouTube
                  onSubmit={(ev) => {
                    ev.preventDefault();

                    if (ev.currentTarget.url.value === "") return;

                    if (videoIds().length === 16)
                      return console.warn("Maximum number of videos reached.");

                    const parsed = parseYouTubeUrl(ev.currentTarget.url.value);
                    if (parsed.type !== "video") return;

                    setVideoIds((prev) => [...prev, parsed.id]);
                    ev.currentTarget.url.value = "";
                  }}
                  Action={<button type="submit">Watch</button>}
                ></WatchVideoFromYouTube>
              </div>
            }
            keyed
          >
            {(data) => (
              <div
                class={`grid gap-2 ${squareDivisionsMap.get(divisions())} justify-items-center w-full h-full aspect-ratio-video`}
              >
                {data.map((videoId) => (
                  <Player
                    videoId={videoId}
                    rating={
                      liked()
                        ? "like"
                        : (ratings()?.get(videoId)?.rating ?? null)
                    }
                    onClickLike={async () => {
                      setLiked(true);
                      try {
                        await like(videoId);
                      } catch {
                        return setLiked(false);
                      }
                    }}
                  />
                ))}
              </div>
            )}
          </Show>
        </div>
      </div>
    </div>
  );
};
export default Watch;
