import { cache, createAsync, type RouteDefinition } from "@solidjs/router";
import { createSignal, For, Show } from "solid-js";
import {
  listMyChannels,
  type MyChannelsRequest,
  useYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube";

const fetchChannels = cache(
  async (client: YouTubeApiClient, params: MyChannelsRequest["GET"]) => {
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
} satisfies RouteDefinition;

const Index = () => {
  const apiClient = useYouTubeApiClient();
  const [videoId, setVideoId] = createSignal("");
  const channels = createAsync(
    () => fetchChannels(apiClient, { part: ["snippet"], maxResults: 50 }),
    { deferStream: true },
  );

  return (
    <Show when={channels()}>
      {(data) => (
        <>
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
        </>
      )}
    </Show>
  );
};
export default Index;
