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
  type VideoRatingRequest,
  type VideoRatingResponse,
} from "~/libs/api/youtube";
import { Header } from "~/uis/header";
import { getLoginStatus, LoginButton, LogoutButton } from "~/uis/login-button";
import { WatchVideoFromYouTube } from "~/uis/watch-video-from-you-tube";

const Player = clientOnly(() =>
  import("./player").then(({ Player }) => ({ default: Player })),
);

const fetchRating = cache(async (params: VideoRatingRequest["GET"]) => {
  "use server";
  const { youtubeApi } = getRequestEvent()!.locals;
  let rating: VideoRatingResponse["GET"];
  try {
    rating = await getVideoRating(youtubeApi)(params);
  } catch {
    return null;
  }
  return rating;
}, "rating");

const likeAction = action(async (id: string) => {
  "use server";
  const { youtubeApi } = getRequestEvent()!.locals;
  await postVideoRating(youtubeApi)({ id, rating: "like" });
  return null;
});

type Params = { videoId: string };

export const routes = {
  load: () => {
    const [{ videoId }] = useSearchParams<Params>();
    if (!videoId) return null;

    return (
      fetchRating({ id: videoId })
        // https://github.com/solidjs/solid-router/issues/399
        .catch((err) => {
          console.error(err);
          return null;
        })
    );
  },
} satisfies RouteDefinition;

const Watch = () => {
  const [searchParams, setSearchParams] = useSearchParams<Params>();
  const navigate = useNavigate();

  const [videoId, setVideoId] = createSignal(searchParams.videoId);
  const [liked, setLiked] = createSignal(false);
  const isLoggedIn = createAsync(() => getLoginStatus(), { deferStream: true });
  const ratingData = createAsync(
    async () =>
      searchParams.videoId ? fetchRating({ id: searchParams.videoId }) : null,
    {
      deferStream: true,
    },
  );

  createEffect(() => {
    if (videoId() === undefined) return;
    if (videoId() === searchParams.videoId) return;
    setSearchParams({ videoId: videoId() });
  });

  const like = useAction(likeAction);

  return (
    <>
      <Header
        LeftSide={
          <Show when={videoId()}>
            <WatchVideoFromYouTube
              onSubmit={(ev) => {
                ev.preventDefault();
                
                const videoId =
                  new URL(ev.currentTarget.url.value).searchParams.get("v") ??
                  "";
                
                if (
                  (ev.submitter as HTMLButtonElement).name === "openCurrentPage"
                )
                  setVideoId(videoId);
                else {
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
        when={videoId()}
        fallback={
          <div class="grid justify-center items-center w-full aspect-ratio-video ">
            <WatchVideoFromYouTube
              onSubmit={(ev) => {
                ev.preventDefault();
                
                const videoId =
                  new URL(ev.currentTarget.url.value).searchParams.get("v") ??
                  "";
                
                setVideoId(videoId);
                ev.currentTarget.url.value = "";
              }}
              Action={<button type="submit">Watch</button>}
            ></WatchVideoFromYouTube>
          </div>
        }
        keyed
      >
        {(videoId) => (
          <Player
            videoId={videoId}
            rating={liked() ? "like" : (ratingData()?.rating ?? null)}
            onClickLike={async () => {
              setLiked(true);
              try {
                await like(videoId);
              } catch {
                return setLiked(false);
              }
            }}
          />
        )}
      </Show>
    </>
  );
};
export default Watch;
