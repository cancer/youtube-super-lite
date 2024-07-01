import {
  cache,
  createAsync,
  type RouteDefinition,
  useParams,
} from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { Show } from "solid-js";
import {
  getVideoRating,
  type VideoRatingRequest,
  type VideoRatingResponse,
} from "~/libs/api/youtube";
import { LikeButton } from "./like-button";

const Player = clientOnly(() =>
  import("./player").then(({ Player }) => ({ default: Player })),
);

const fetchRating = cache(async (params: VideoRatingRequest["GET"]) => {
  "use server";
  let rating: VideoRatingResponse["GET"];
  try {
    rating = await getVideoRating(params);
  } catch {
    return null;
  }
  return rating;
}, "rating");

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

  const like = (videoId: string) => console.log("liked", videoId);

  return (
    <Show when={params.id} fallback="Need videoId." keyed>
      {(videoId) => (
        <div class="w-full">
          <Player videoId={videoId} />
          <Show when={ratingData()}>
            {(data) => (
              <LikeButton
                liked={data().rating === "like"}
                onClick={() => like(videoId)}
              />
            )}
          </Show>
        </div>
      )}
    </Show>
  );
};
export default Watch;
