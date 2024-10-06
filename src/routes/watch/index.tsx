import {
  action,
  cache,
  createAsync,
  type RouteDefinition,
  useAction,
  useNavigate,
  useSearchParams,
} from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { createEffect, createSignal, Show } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import {
  getVideoRating,
  postVideoRating,
  type VideoRatingResponse,
} from "~/libs/api/youtube";
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
  const navigate = useNavigate();

  const [videoIds, setVideoIds] = createSignal<string[]>(
    searchParams.videoIds?.split(",") ?? [],
  );
  const [liked, setLiked] = createSignal(false);
  const isLoggedIn = createAsync(() => getLoginStatus(), { deferStream: true });
  const ratings = createAsync(async () => fetchRatings({ ids: videoIds() }), {
    deferStream: true,
  });

  createEffect(() => {
    if (videoIds().length === 0) return;
    setSearchParams({ videoIds: videoIds().join(",") });
  });

  const like = useAction(likeAction);

  return (
    <>
      <Header
        LeftSide={
          <Show when={videoIds().length > 0}>
            <WatchVideoFromYouTube
              onSubmit={(ev) => {
                ev.preventDefault();

                if (ev.currentTarget.url.value === "") return;

                const videoId =
                  new URL(ev.currentTarget.url.value).searchParams.get("v") ??
                  "";
                if (
                  (ev.submitter as HTMLButtonElement).name === "openCurrentPage"
                ) {
                  setVideoIds((prev) => [...prev, videoId]);
                } else {
                  const params = new URLSearchParams({ videoId });
                  navigate(`/watch/?${params.toString()}`);
                }
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
      <Show
        when={videoIds().length > 0 && videoIds()}
        fallback={
          <div class="grid justify-center items-center w-full aspect-ratio-video ">
            <WatchVideoFromYouTube
              onSubmit={(ev) => {
                ev.preventDefault();

                if (ev.currentTarget.url.value === "") return;

                const videoId =
                  new URL(ev.currentTarget.url.value).searchParams.get("v") ??
                  "";
                setVideoIds((prev) => [...prev, videoId]);
                ev.currentTarget.url.value = "";
              }}
              Action={<button type="submit">Watch</button>}
            ></WatchVideoFromYouTube>
          </div>
        }
        keyed
      >
        {(data) => (
          <div class="grid">
            {data.map((videoId) => (
              <Player
                videoId={videoId}
                rating={
                  liked() ? "like" : (ratings()?.get(videoId)?.rating ?? null)
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
    </>
  );
};
export default Watch;
