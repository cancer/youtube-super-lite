import { action, redirect, useAction, useNavigate } from "@solidjs/router";
import { type VoidComponent } from "solid-js";
import { createAuthClient, revokeToken } from "~/libs/api/auth";
import { createAuthTokensClient } from "~/libs/auth-tokens/client";
import { getSession } from "~/libs/session";

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

export const Header: VoidComponent = () => {
  const navigate = useNavigate();
  const logout = useAction(logoutAction);
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
        <button onClick={logout}>Logout</button>
      </div>
    </div>
  );
};
