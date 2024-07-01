import { cache, createAsync, type RouteDefinition } from "@solidjs/router";
import { For, Show } from "solid-js";
import { listMyChannels, type MyChannelsRequest } from "~/libs/api/youtube";

const fetchChannels = cache(async (params: MyChannelsRequest["GET"]) => {
  "use server";
  return listMyChannels(params);
}, "channels");

export const route = {
  load: () => {
    return (
      fetchChannels({
        part: ["snippet"],
        maxResults: 50,
      })
        // https://github.com/solidjs/solid-router/issues/399
        .catch((err) => {
          console.error(err);
          return null;
        })
    );
  },
} satisfies RouteDefinition;

//
// !!! TODO: サブリクエストが50超えてしまっているのでなんとかしないといけない !!!
//
const Index = () => {
  const channels = createAsync(
    () => fetchChannels({ part: ["snippet"], maxResults: 50 }),
    { deferStream: true },
  );

  return (
    <div class="grid grid-cols-[min-content_auto] gap-8">
      <Show when={channels()}>
        {(data) => (
          <ul class="flex flex-col gap-2 list-none w-8 p-0">
            <For each={data().items}>
              {(channel) => (
                <li class="w-full aspect-square">
                  <a href={`/channels/${channel.snippet.resourceId.channelId}`}>
                    <img
                      src={channel.snippet.thumbnails.default.url}
                      alt={channel.snippet.title}
                      class="w-full rounded-full"
                    />
                  </a>
                </li>
              )}
            </For>
          </ul>
        )}
      </Show>
    </div>
  );
};
export default Index;
