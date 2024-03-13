import { cache, createAsync, type RouteDefinition } from "@solidjs/router";
import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { isServer } from "solid-js/web";
import { Redirect } from "~/components/redirect";
import { result } from "~/libs/api/result";
import {
  isTokenExpired,
  listMyChannels,
  type MyChannelsRequest,
  useYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube";

const fetchChannels = cache(
  async (client: YouTubeApiClient, params: MyChannelsRequest["GET"]) => {
    "use server";
    return result(() => listMyChannels(client)(params), { log: console.error });
  },
  "channels",
);

export const route = {
  load: () => {
    const apiClient = useYouTubeApiClient();
    return fetchChannels(apiClient, {
      part: ["snippet"],
      maxResults: 50,
    });
  },
} satisfies RouteDefinition;

const Index = () => {
  const apiClient = useYouTubeApiClient();
  const [videoId, setVideoId] = createSignal("");
  const channels = createAsync(
    () => fetchChannels(apiClient, { part: ["snippet"], maxResults: 50 }),
    { deferStream: true },
  );

  const needLogin = createMemo(() => {
    if (!channels()) return false;
    if (!channels()!.isError) return false;
    if (!isTokenExpired(channels()!.error)) return false;
    return true;
  });

  return (
    <Switch>
      <Match when={isServer && needLogin()}>
        <Redirect path="/login?redirect_to=/" />
      </Match>
      <Match when={channels()?.isError}>
        <div>{(channels()!.error as Error).message}</div>
      </Match>
      <Match when={channels()?.isSuccess}>
        <form
          onSubmit={(ev) => {
            ev.preventDefault();
            const url = new URL(
              (ev.currentTarget as HTMLFormElement).url.value,
            );
            setVideoId(url.searchParams.get("v") ?? "");
          }}
        >
          <input class="w-2xl h-10 text-xl" type="text" name="url" />
          <button type="submit">Watch</button>
        </form>
        <Show when={channels()?.data}>
          {(data) => (
            <ul class="flex list-none">
              <For each={data().items}>
                {(channel) => (
                  <li>
                    <a
                      href={`/channels/${channel.snippet.resourceId.channelId}`}
                    >
                      <img
                        src={channel.snippet.thumbnails.default.url}
                        alt={channel.snippet.title}
                        class="w-10"
                      />
                    </a>
                  </li>
                )}
              </For>
            </ul>
          )}
        </Show>
      </Match>
    </Switch>
  );
};
export default Index;
