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
    return fetchRating({ id: videoId });
  },
} satisfies RouteDefinition;

const Watch = () => {
  const { id: videoId } = useParams<Params>();
  const rating = createAsync(async () => fetchRating({ id: videoId }), {
    deferStream: true,
  });

  const like = () => console.log("liked", videoId);

  return (
    <Show when={videoId !== ""} fallback="Need videoId.">
      <div class="w-full">
        <Player videoId={videoId} />
        <LikeButton
          liked={rating()?.rating === "like"}
          onClick={() => like()}
        />
      </div>
    </Show>
  );
};
export default Watch;
