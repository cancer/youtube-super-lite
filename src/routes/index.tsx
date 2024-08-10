import { cache, createAsync, type RouteDefinition } from "@solidjs/router";
import { For, Show } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import { listMyChannels, type MyChannelsRequest } from "~/libs/api/youtube";
import { Subscription } from "~/libs/api/youtube/types";
import { createAuthTokensClient } from "~/libs/auth-tokens/client";
import { getSession } from "~/libs/session";

const fetchChannels = cache(async (params: MyChannelsRequest["GET"]) => {
  "use server";
  let channels: Subscription[] = [];
  try {
    const { items } = await listMyChannels(params);
    return items;
  } catch (err) {
    console.debug(err);
    return [];
  }
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
          console.debug(err);
          return [];
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
    <div class="grid grid-cols-[min-content_auto] gap-8 w-full overflow-x-auto">
      <Show when={channels()}>
        {(data) => (
          <ul class="flex gap-2 list-none p-0">
            <For each={data()}>
              {(channel) => (
                <li class="w-8 aspect-square">
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
