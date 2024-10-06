import {
  cache,
  createAsync,
  type RouteDefinition,
  useNavigate,
} from "@solidjs/router";
import { For, Show } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import { listMyChannels, type MyChannelsRequest } from "~/libs/api/youtube";
import { Header } from "~/uis/header";
import { getLoginStatus, LoginButton, LogoutButton } from "~/uis/login-button";
import { WatchVideoFromYouTube } from "~/uis/watch-video-from-you-tube";

const fetchChannels = cache(async (params: MyChannelsRequest["GET"]) => {
  "use server";
  const { youtubeApi } = getRequestEvent()!.locals;
  try {
    const { items } = await listMyChannels(youtubeApi)(params);
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
  const navigate = useNavigate();

  const isLoggedIn = createAsync(() => getLoginStatus(), { deferStream: true });
  const channels = createAsync(
    () => fetchChannels({ part: ["snippet"], maxResults: 50 }),
    { deferStream: true },
  );

  return (
    <>
      <Header
        LeftSide={
          <WatchVideoFromYouTube
            onSubmit={(ev) => {
              ev.preventDefault();

              const videoId =
                new URL(ev.currentTarget.url.value).searchParams.get("v") ?? "";
              const params = new URLSearchParams({ videoIds: videoId });

              navigate(`/watch/?${params.toString()}`);
              ev.currentTarget.url.value = "";
            }}
            Action={<button type="submit">Watch</button>}
          ></WatchVideoFromYouTube>
        }
        RightSide={
          <Show when={isLoggedIn()} fallback={<LoginButton />}>
            <LogoutButton />
          </Show>
        }
      />
      <div class="grid grid-cols-[min-content_auto] gap-8 w-full overflow-x-auto">
        <Show when={channels()}>
          {(data) => (
            <ul class="flex gap-2 list-none p-0">
              <For each={data()}>
                {(channel) => (
                  <li class="w-8 aspect-square">
                    <a
                      href={`/channels/${channel.snippet.resourceId.channelId}`}
                    >
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
    </>
  );
};
export default Index;
