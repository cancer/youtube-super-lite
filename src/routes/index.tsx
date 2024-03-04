import { cache, createAsync } from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { createSignal, For, Show } from "solid-js";
import {
  listMyChannels,
  type SubscriptionsRequest,
  useYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube";

const Player = clientOnly(() =>
  import("~/components/player").then(({ Player }) => ({ default: Player })),
);

const fetchChannels = cache(
  async (client: YouTubeApiClient, params: SubscriptionsRequest["GET"]) => {
    "use server";
    return listMyChannels(client)(params);
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
};

const Index = () => {
  const apiClient = useYouTubeApiClient();
  const channels = createAsync(() =>
    fetchChannels(apiClient, {
      part: ["contentDetails", "snippet"],
      maxResults: 50,
    }),
  );
  const [videoId, setVideoId] = createSignal("");
  return (
    <main class="bg-black h-full">
      <form
        onSubmit={(ev) => {
          ev.preventDefault();
          const url = new URL((ev.currentTarget as HTMLFormElement).url.value);
          setVideoId(url.searchParams.get("v") ?? "");
        }}
      >
        <input class="w-2xl h-10 text-xl" type="text" name="url" />
        <button type="submit">Watch</button>
      </form>
      <Player videoId={videoId()} />
      <Show when={channels()}>
        {(data) => (
          <ul class="flex list-none">
            <For each={data().items}>
              {(channel) => (
                <li>
                  <a href={`/channels/${channel.snippet.resourceId.channelId}`}>
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
  );
};
export default Index;
