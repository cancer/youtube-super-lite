import { redirect } from "@solidjs/router";
import type { APIEvent } from "@solidjs/start/server";
import { getCookie } from "vinxi/http";
import {
  createAuthApiClient,
  exchangeTokens,
  refreshAccessToken,
  revokeToken,
} from "~/libs/api/auth";
import { serialize } from "~/libs/cookie";

const stateKey = "ytp_state";
export const GET = async ({ request, locals: { env, auth } }: APIEvent) => {
  "use server";

  const authApiClient = createAuthApiClient({
    clientId: env.GAUTH_CLIENT_ID!,
    clientSecret: env.GAUTH_CLIENT_SECRET!,
  });
  const url = new URL(request.url);

  // for refresh
  let tokens;
  try {
    tokens = await auth.get();
  } catch {}
  if (tokens) {
    console.log(`Tokens retrieved. ${JSON.stringify(tokens)}`);

    let refreshed;
    try {
      refreshed = await refreshAccessToken(authApiClient)(tokens.refreshToken);
    } catch (err) {
      console.error("Failed to refresh access token: ", err);

      await revokeToken(authApiClient)(tokens.refreshToken).catch((e) =>
        console.error("Failed to revoke tokens: ", e),
      );
      await auth
        .clear()
        .catch((e: Error) => console.error("Failed to clear session: ", e));
      return redirect(`/login${url.search}`);
    }

    try {
      await auth.set({
        accessToken: refreshed.accessToken,
        refreshToken: refreshed.refreshToken,
        expiresAt: Date.now() + refreshed.expiresIn * 1000,
      });
    } catch (err) {
      console.error("Failed to set tokens to session: ", err);
      return redirect("/error", {
        status: 500,
        statusText: (err as Error).message,
      });
    }

    return redirect(url.searchParams.get("redirect_to") ?? "/");
  }

  // for callback
  if (url.searchParams.has("state")) {
    if (url.searchParams.get("state") !== getCookie(stateKey)) {
      console.error("Invalid state");
      return redirect("/error", { status: 400 });
    }

    if (!url.searchParams.has("code")) {
      console.error("Invalid code");
      return redirect("/error", { status: 400 });
    }

    console.info("Attempt to exchange tokens.");

    let tokens;
    try {
      tokens = await exchangeTokens(authApiClient)({
        code: url.searchParams.get("code")!,
        redirectUri: `${url.origin}/login`,
      });
    } catch (err) {
      console.error("Failed to exchange tokens: ", err);
      return redirect("/error", {
        status: 401,
        statusText: (err as Error).message,
      });
    }

    // revoke tokens and re-login if refresh_token is missing
    if (tokens.refreshToken === "") {
      await revokeToken(authApiClient)(tokens.accessToken).catch((e) =>
        console.error("Failed to revoke tokens: ", e),
      );
      return redirect(`/login${url.search}`);
    }

    try {
      await auth.set({
        accessToken: tokens!.accessToken,
        refreshToken: tokens!.refreshToken,
        expiresAt: Date.now() + tokens!.expiresIn * 1000,
      });
    } catch (err) {
      console.error("Failed to set tokens to session: ", err);
      return redirect("/error", {
        status: 500,
        statusText: (err as Error).message,
      });
    }

    return redirect("/", { status: 302 });
  }

  // for initial login
  console.info("Attempt to login");
  const state = crypto.randomUUID();
  const params = new URLSearchParams();

  params.append("redirect_uri", `${url.origin}/login`);
  params.append("state", state);
  params.append("client_id", env.GAUTH_CLIENT_ID!);
  params.append("response_type", "code");
  params.append(
    "scope",
    "https://www.googleapis.com/auth/youtube.readonly https://www.googleapis.com/auth/youtube.force-ssl",
  );
  params.append("access_type", "offline");
  params.append("prompt", "select_account");

  return redirect(`https://accounts.google.com/o/oauth2/v2/auth?${params}`, {
    status: 302,
    headers: {
      "Set-Cookie": serialize(stateKey, state, {
        "Max-Age": 300,
        HttpOnly: true,
        Path: "/",
        SameSite: "Lax",
        Secure: true,
      }),
    },
  });
};
