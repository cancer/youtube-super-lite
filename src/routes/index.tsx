import {
  cache,
  createAsync,
  type RouteDefinition,
  useNavigate,
} from "@solidjs/router";
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

const Index = () => {
  const navigate = useNavigate();
  const channels = createAsync(
    () => fetchChannels({ part: ["snippet"], maxResults: 50 }),
    { deferStream: true },
  );

  return (
    <>
      <form
        onSubmit={(ev) => {
          ev.preventDefault();
          const url = new URL((ev.currentTarget as HTMLFormElement).url.value);
          navigate(`/watch/${url.searchParams.get("v") ?? ""}`);
        }}
      >
        From YT URL: <input class="w-2xl h-10 text-xl" type="text" name="url" />
        <button type="submit">Watch</button>
      </form>
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
    </>
  );
};
export default Index;
