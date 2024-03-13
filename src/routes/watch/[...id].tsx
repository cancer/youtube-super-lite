import {
  cache,
  createAsync,
  type RouteDefinition,
  useParams,
} from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { Show } from "solid-js";
import { LikeButton } from "~/components/like-button";
import {
  getVideoRating,
  useYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube";

const Player = clientOnly(() =>
  import("~/components/player").then(({ Player }) => ({ default: Player })),
);

const fetchRating = cache(
  async (client: YouTubeApiClient, params: { id: string }) => {
    "use server";
    return getVideoRating(client)(params);
  },
  "rating",
);

export const routes = {
  load: () => {
    const { id: videoId } = useParams<Params>();
    const apiClient = useYouTubeApiClient();
    return fetchRating(apiClient, { id: videoId });
  },
} satisfies RouteDefinition;

type Params = { id: string };
const Watch = () => {
  const { id: videoId } = useParams<Params>();
  const apiClient = useYouTubeApiClient();
  const rating = createAsync(
    async () => fetchRating(apiClient, { id: videoId }),
    { deferStream: true },
  );

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
