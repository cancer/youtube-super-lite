import {
  cache,
  createAsync,
  redirect,
  type RouteDefinition,
} from "@solidjs/router";
import { For, Show } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import {
  listMyChannels,
  type MyChannelsRequest,
  type MyChannelsResponse,
} from "~/libs/api/youtube";
import { isTokenExpired } from "~/libs/api/youtube/errors";

const fetchChannels = cache(async (params: MyChannelsRequest["GET"]) => {
  "use server";
  let channels: MyChannelsResponse["GET"];
  try {
    channels = await listMyChannels(params);
  } catch (err) {
    if (isTokenExpired(err)) {
      const event = getRequestEvent();
      const redirectTo = event ? new URL(event.request.url).pathname : "/";
      throw redirect(`/login?redirect_to=${redirectTo}`);
    }
    throw err;
  }
  return channels;
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
    <div class="grid grid-cols-[min-content_auto] gap-8 w-full overflow-x-auto">
      <Show when={channels()}>
        {(data) => (
          <ul class="flex gap-2 list-none p-0">
            <For each={data().items}>
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
