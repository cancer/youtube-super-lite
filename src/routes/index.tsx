import { cache, createAsync, type RouteDefinition } from "@solidjs/router";
import { clientOnly, HttpHeader, HttpStatusCode } from "@solidjs/start";
import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { isServer } from "solid-js/web";
import {
  listMyChannels,
  type MyChannelsRequest,
  TokenExpiredError,
  useYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube";

const Player = clientOnly(() =>
  import("~/components/player").then(({ Player }) => ({ default: Player })),
);

const fetchChannels = cache(
  async (client: YouTubeApiClient, params: MyChannelsRequest["GET"]) => {
    "use server";

    // Since exceptions thrown inside cache() are not caught by ErrorBoundary
    try {
      const data = await listMyChannels(client)(params);
      return {
        isSuccess: true,
        isError: false,
        data,
        error: null,
      };
    } catch (error) {
      console.error(error);
      return {
        isSuccess: false,
        isError: true,
        data: null,
        error,
      };
    }
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
  const channels = createAsync(() =>
    fetchChannels(apiClient, { part: ["snippet"], maxResults: 50 }),
  );
  const [videoId, setVideoId] = createSignal("");

  const needLogin = createMemo(() => {
    if (!isServer) return false;
    const _channels = channels();
    if (!_channels) return false;
    if (!_channels.isError) return false;
    if (!(_channels.error instanceof TokenExpiredError)) return false;
    return true;
  });

  return (
    <Switch>
      <Match when={needLogin()}>
        <HttpStatusCode code={302} />
        <HttpHeader name="Location" value="/login" />
      </Match>
      <Match when={channels()?.isError}>
        <div>{(channels()!.error as Error).message}</div>
      </Match>
      <Match when={channels()?.isSuccess}>
        <main class="bg-black h-full">
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
          <Player videoId={videoId()} />
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
        </main>
      </Match>
    </Switch>
  );
};
export default Index;
