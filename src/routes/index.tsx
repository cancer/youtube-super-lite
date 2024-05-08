import {
  A,
  action,
  cache,
  createAsync,
  redirect,
  type RouteDefinition,
  useAction,
  useNavigate,
} from "@solidjs/router";
import { createMemo, For, Match, Show, Switch } from "solid-js";
import { createAuthClient, revokeToken } from "~/libs/api/auth";
import {
  LatestVideoListRequestGet,
  LatestVideoListResponseGet,
  listLatestVideos,
  listMyChannels,
  type MyChannelsRequest,
} from "~/libs/api/youtube";
import { createAuthTokensClient } from "~/libs/auth-tokens/client";
import {
  formatDurationTime,
  getHourDiff,
  startOfDay,
  subtractDays,
} from "~/libs/datetime";
import { getSession } from "~/libs/session";

const fetchChannels = cache(async (params: MyChannelsRequest["GET"]) => {
  "use server";
  return listMyChannels(params);
}, "channels");

const fetchLatestLiveStreaming = cache(
  async (params: LatestVideoListRequestGet) => {
    "use server";
    const videos = await listLatestVideos(params);
    const [live, others] = videos.items.reduce(
      (acc: LatestVideoListResponseGet["items"][number][][], item) => {
        if (item.snippet.liveBroadcastContent === "live") {
          acc[0].push(item);
          return acc;
        }
        // ただの動画
        if (item.liveStreamingDetails === undefined) return acc;
        acc[1].push(item);
        return acc;
      },
      [[], []],
    );
    return [...live, ...others];
  },
  "latestVideos",
);

const logoutAction = action(async () => {
  "use server";
  const authClient = createAuthClient({
    clientId: process.env.GAUTH_CLIENT_ID!,
    clientSecret: process.env.GAUTH_CLIENT_SECRET!,
  });
  const authTokensClient = createAuthTokensClient(() =>
    getSession(process.env.SESSION_SECRET!),
  );

  const tokens = await authTokensClient.get();
  if (tokens !== null) await revokeToken(authClient)(tokens.accessToken);

  await authTokensClient.clear();
  throw redirect("/");
});

export const route = {
  load: () => {
    return (
      Promise.all([
        fetchChannels({
          part: ["snippet"],
          maxResults: 50,
        }),
        fetchLatestLiveStreaming({
          maxResults: 50,
          publishedAfter: startOfDay(subtractDays(new Date(), 1)),
        }),
      ])
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
  const thumbnailsMap = createMemo(() => {
    if (!channels()) return {};
    return channels()!.items.reduce(
      (acc, channel) => {
        acc[channel.snippet.resourceId.channelId] =
          channel.snippet.thumbnails.default.url;
        return acc;
      },
      {} as Record<string, string>,
    );
  });
  const latestVideos = createAsync(
    () =>
      fetchLatestLiveStreaming({
        maxResults: 50,
        publishedAfter: startOfDay(subtractDays(new Date(), 1)),
      }),
    { deferStream: true },
  );

  const logout = useAction(logoutAction);

  return (
    <div class="grid grid-cols-[min-content_auto] gap-8">
      <div class="col-span-full flex justify-between">
        <form
          onSubmit={(ev) => {
            ev.preventDefault();
            const url = new URL(
              (ev.currentTarget as HTMLFormElement).url.value,
            );
            navigate(`/watch/${url.searchParams.get("v") ?? ""}`);
          }}
        >
          From YT URL:{" "}
          <input class="w-2xl h-10 text-xl" type="text" name="url" />
          <button type="submit">Watch</button>
        </form>
        <button onClick={logout}>Logout</button>
      </div>
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
      <Show when={latestVideos()}>
        {(data) => (
          <ul class="w-full flex flex-wrap gap-4 overflow-x-hidden list-none p-0">
            <For each={data()}>
              {({ id, snippet, liveStreamingDetails, contentDetails }) => (
                <li class="w-min object-contain grid gap-2">
                  <div class="grid grid-cols-[auto_min-content_0.2rem] grid-rows-[auto_min-content_0.2rem]">
                    <A
                      href={`/watch/${id}`}
                      class="hover-opacity-80 col-span-full row-span-full"
                    >
                      <img
                        src={snippet.thumbnails.medium.url}
                        alt={snippet.title}
                        width={snippet.thumbnails.medium.width}
                        classList={{
                          "border-red": snippet.liveBroadcastContent === "live",
                          "border-4": snippet.liveBroadcastContent === "live",
                          "border-solid":
                            snippet.liveBroadcastContent === "live",
                        }}
                      />
                    </A>
                    <span class="min-h-4 col-start-2 row-start-2 z-1">
                      <Show when={snippet.liveBroadcastContent === "none"}>
                        <span class="bg-black rounded p-1 text-xs">
                          {formatDurationTime(contentDetails.duration)}
                        </span>
                      </Show>
                    </span>
                  </div>
                  <A
                    href={`/watch/${id}`}
                    class="flex gap-xs items-center color-white decoration-none hover-decoration-underline"
                  >
                    <img
                      src={thumbnailsMap()[snippet.channelId]}
                      alt={snippet.channelTitle}
                      class="w-8 h-8 rounded-full"
                    />
                    <span class="m0 line-clamp-2">{snippet.title}</span>
                  </A>
                  <p class="flex justify-between m0 text-xs">
                    <span class="text-stone-400 line-clamp-1">
                      {snippet.channelTitle}
                    </span>
                    <Switch>
                      <Match when={snippet.liveBroadcastContent === "live"}>
                        <span class="bg-red rounded w-min pl-2 pr-2">Live</span>
                      </Match>
                      <Match when={snippet.liveBroadcastContent === "upcoming"}>
                        <span class="bg-stone-500 rounded w-min pl-2 pr-2 text-nowrap">
                          {new Intl.DateTimeFormat("ja-JP", {
                            month: "numeric",
                            day: "numeric",
                            hour: "numeric",
                            minute: "numeric",
                          }).format(
                            new Date(liveStreamingDetails.scheduledStartTime),
                          )} 〜
                        </span>
                      </Match>
                      <Match when={snippet.liveBroadcastContent === "none"}>
                        {new Intl.RelativeTimeFormat("ja-JP").format(
                          getHourDiff(
                            new Date(liveStreamingDetails.actualEndTime),
                            new Date(),
                          ),
                          "hour",
                        )}に配信済み
                      </Match>
                    </Switch>
                  </p>
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
