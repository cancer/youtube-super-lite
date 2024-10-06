import {
  action,
  cache,
  createAsync,
  type RouteDefinition,
  useAction,
  useParams,
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

type Params = { id: string };

export const routes = {
  load: () => {
    const { id: videoId } = useParams<Params>();
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
  const params = useParams<Params>();
  const ratingData = createAsync(async () => fetchRating({ id: params.id }), {
    deferStream: true,
  });
  const like = useAction(likeAction);
  const [liked, setLiked] = createSignal(false);

  return (
    <Show when={params.id} fallback="Need videoId." keyed>
      {(videoId) => (
        <div class="w-full">
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
        </div>
      )}
    </Show>
  );
};
export default Watch;
