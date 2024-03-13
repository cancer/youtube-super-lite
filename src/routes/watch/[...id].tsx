import { cache, createAsync, useLocation, useParams } from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { createMemo, Match, Switch } from "solid-js";
import { isServer } from "solid-js/web";
import { LikeButton } from "~/components/like-button";
import { Redirect } from "~/components/redirect";
import { result } from "~/libs/api/result";
import {
  isTokenExpired,
  useYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube";

const Player = clientOnly(() =>
  import("~/components/player").then(({ Player }) => ({ default: Player })),
);

const fetchRating = cache(
  async (client: YouTubeApiClient, params: { id: string }) => {
    "use server";
    return result(
      () =>
        client
          .request({
            uri: "/videos/getRating",
            method: "GET",
            params,
          })
          .then((res) => ((res as any).items[0]?.rating as string) ?? ""),
      { log: () => {} },
    );
  },
  "rating",
);

type Params = { id: string };
const Watch = () => {
  const { pathname } = useLocation();
  const { id: videoId } = useParams<Params>();
  const apiClient = useYouTubeApiClient();
  const rating = createAsync(
    async () => {
      return fetchRating(apiClient, { id: videoId });
    },
    { deferStream: true },
  );

  const needLogin = createMemo(() => {
    if (!rating()) return false;
    if (!rating()!.isError) return false;
    if (!isTokenExpired(rating()!.error)) return false;
    return true;
  });

  const like = () => console.log("liked", videoId);

  return (
    <Switch>
      <Match when={isServer && needLogin()}>
        <Redirect path={`/login?redirect_to=${pathname}`} />
      </Match>
      <Match when={rating()?.isError}>
        <div>{(rating()!.error as Error).message}</div>
      </Match>
      <Match when={rating()?.isSuccess && videoId === ""}>
        <div>Need videoId.</div>
      </Match>
      <Match when={rating()?.isSuccess && videoId !== ""}>
        <div class="w-full">
          <Player videoId={videoId} />
          <LikeButton
            liked={rating()!.data === "like"}
            onClick={() => like()}
          />
        </div>
      </Match>
    </Switch>
  );
};
export default Watch;
