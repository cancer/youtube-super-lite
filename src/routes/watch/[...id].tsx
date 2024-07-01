import {
  cache,
  createAsync,
  type RouteDefinition,
  useParams,
} from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { Show } from "solid-js";
import { LikeButton } from "~/components/like-button";
import { getVideoRating } from "~/libs/api/youtube";

const Player = clientOnly(() =>
  import("~/components/player").then(({ Player }) => ({ default: Player })),
);

const fetchRating = cache((params) => {
  "use server";
  return getVideoRating(params);
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
  const rating = createAsync(async () => fetchRating({ id: params.id }), {
    deferStream: true,
  });

  const like = (videoId: string) => console.log("liked", videoId);

  return (
    <Show when={params.id} fallback="Need videoId." keyed>
      {(videoId) => (
        <div class="w-full">
          <Player videoId={videoId} />
          <LikeButton
            liked={rating()?.rating === "like"}
            onClick={() => like(videoId)}
          />
        </div>
      )}
    </Show>
  );
};
export default Watch;
