import {
  A,
  action,
  cache,
  createAsync,
  redirect,
  useAction,
  useNavigate,
} from "@solidjs/router";
import { Show, type VoidComponent } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import { createAuthClient, revokeToken } from "~/libs/api/auth";
import { createAuthTokensClient } from "~/libs/auth-tokens/client";
import { getSession } from "~/libs/session";

const getLoginStatus = cache(async () => {
  "use server";
  const ev = getRequestEvent()!;
  const authTokensClient = createAuthTokensClient(() =>
    getSession(ev.locals.env.SESSION_SECRET),
  );
  return (await authTokensClient.get()) !== null;
}, "loginStatus");

const logoutAction = action(async () => {
  "use server";
  const ev = getRequestEvent()!;
  const authClient = createAuthClient({
    clientId: ev.locals.env.GAUTH_CLIENT_ID,
    clientSecret: ev.locals.env.GAUTH_CLIENT_SECRET,
  });
  const authTokensClient = createAuthTokensClient(() =>
    getSession(ev.locals.env.SESSION_SECRET),
  );

  const tokens = await authTokensClient.get();
  if (tokens !== null) await revokeToken(authClient)(tokens.accessToken);

  await authTokensClient.clear();
  throw redirect("/");
});

export const Header: VoidComponent = () => {
  const navigate = useNavigate();
  const logout = useAction(logoutAction);
  const isLoggedIn = createAsync(() => getLoginStatus(), { deferStream: true });

  return (
    <div class="grid">
      <div class="col-span-full flex justify-between">
        <form
          onSubmit={(ev) => {
            ev.preventDefault();
            const url = new URL(ev.currentTarget.url.value);
            navigate(`/watch/${url.searchParams.get("v") ?? ""}`);
            ev.currentTarget.url.value = "";
          }}
        >
          From YT URL:{" "}
          <input class="w-2xl h-10 text-xl" type="text" name="url" />
          <button type="submit">Watch</button>
        </form>
        <Show
          when={isLoggedIn()}
          fallback={
            <button onClick={() => location.assign("/login")}>Login</button>
          }
        >
          <button onClick={logout}>Logout</button>
        </Show>
      </div>
    </div>
  );
};
