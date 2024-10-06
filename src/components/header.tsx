import {
  action,
  cache,
  createAsync,
  redirect,
  useAction,
  useNavigate,
} from "@solidjs/router";
import { Match, Switch, type VoidComponent } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import { createAuthApiClient, revokeToken } from "~/libs/api/auth";

const getLoginStatus = cache(async () => {
  "use server";
  const ev = getRequestEvent()!;
  return (await ev.locals.auth.get()) !== null;
}, "loginStatus");

const logoutAction = action(async () => {
  "use server";
  const ev = getRequestEvent()!;
  const authClient = createAuthApiClient({
    clientId: ev.locals.env.GAUTH_CLIENT_ID,
    clientSecret: ev.locals.env.GAUTH_CLIENT_SECRET,
  });

  const tokens = await ev.locals.auth.get();
  if (tokens !== null) await revokeToken(authClient)(tokens.accessToken);

  await ev.locals.auth.clear();
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
        <Switch fallback={<div />}>
          <Match when={isLoggedIn() === true}>
            <button onClick={logout}>Logout</button>
          </Match>
          <Match when={isLoggedIn() === false}>
            <button onClick={() => location.assign("/login")}>Login</button>
          </Match>
        </Switch>
      </div>
    </div>
  );
};
