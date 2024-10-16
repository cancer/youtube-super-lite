import { action, redirect, useAction } from "@solidjs/router";
import { getRequestEvent } from "solid-js/web";
import { createAuthApiClient, revokeToken } from "~/libs/api/auth";

export const getLoginStatus = async () => {
  "use server";
  const ev = getRequestEvent()!;
  return (await ev.locals.auth.get()) !== null;
};

export const LoginButton = () => {
  return (
    <button onClick={() => window.location.assign("/login")}>Login</button>
  );
};

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
export const LogoutButton = () => {
  const logout = useAction(logoutAction);
  return <button onClick={logout}>Logout</button>;
};
