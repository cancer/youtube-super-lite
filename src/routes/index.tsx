import {
  action,
  cache,
  createAsync,
  redirect,
  type RouteDefinition,
  useAction,
  useNavigate,
} from "@solidjs/router";
import { createMemo, For, Show } from "solid-js";
import { createAuthClient, revokeToken } from "~/libs/api/auth";
import { listMyChannels, type MyChannelsRequest } from "~/libs/api/youtube";
import { createAuthTokensClient } from "~/libs/auth-tokens/client";
import { getSession } from "~/libs/session";

const fetchChannels = cache(async (params: MyChannelsRequest["GET"]) => {
  "use server";
  return listMyChannels(params);
}, "channels");

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
  const navigate = useNavigate();
  const channels = createAsync(
    () => fetchChannels({ part: ["snippet"], maxResults: 50 }),
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
    </div>
  );
};
export default Index;
