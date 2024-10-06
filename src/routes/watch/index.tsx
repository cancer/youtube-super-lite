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
import { createSignal, Show } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import {
  getVideoRating,
  postVideoRating,
  type VideoRatingRequest,
  type VideoRatingResponse,
} from "~/libs/api/youtube";
import { Header } from "~/uis/header";
import { getLoginStatus, LoginButton, LogoutButton } from "~/uis/login-button";
import { MovieOpener } from "~/uis/movie-opener";

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
  const [params] = useSearchParams<Params>();
  const navigate = useNavigate();

  const [liked, setLiked] = createSignal(false);
  const isLoggedIn = createAsync(() => getLoginStatus(), { deferStream: true });
  const ratingData = createAsync(
    async () => (params.videoId ? fetchRating({ id: params.videoId }) : null),
    {
      deferStream: true,
    },
  );

  const like = useAction(likeAction);

  return (
    <>
      <Header
        LeftSide={null}
        RightSide={
          <Show when={isLoggedIn()} fallback={<LoginButton />}>
            <LogoutButton />
          </Show>
        }
      />
      <Show
        when={params.videoId}
        fallback={
          <div class="grid justify-center items-center w-full aspect-ratio-video ">
            <MovieOpener
              openVideo={(videoId) => navigate(`/watch/?videoId=${videoId}`)}
            />
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
