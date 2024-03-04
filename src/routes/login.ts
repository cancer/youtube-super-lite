import { redirect } from "@solidjs/router";
import { type APIEvent } from "@solidjs/start/server";
import { getCookie } from "vinxi/http";
import { serialize } from "~/libs/cookie";
import { createAuthClient, exchangeTokens } from "~/libs/api/auth";

const stateKey = "ytp_state";
export const GET = async ({ request }: APIEvent) => {
  "use server";

  const url = new URL(request.url);
  if (url.searchParams.has("state")) {
    if (url.searchParams.get("state") !== getCookie(stateKey))
      return redirect("/error");

    if (!url.searchParams.has("code")) return redirect("/error");

    const authClient = createAuthClient({
      clientId: process.env.GAUTH_CLIENT_ID!,
      clientSecret: process.env.GAUTH_CLIENT_SECRET!,
    });
    let tokens;
    try {
      tokens = await exchangeTokens(authClient)({
        code: url.searchParams.get("code")!,
        redirectUri: `${url.origin}/login`,
      });
    } catch (err) {
      console.log(err);
      return redirect("/error", {
        status: 401,
        statusText: (err as Error).message,
      });
    }

    return redirect("/", {
      status: 302,
      headers: {
        "Set-Cookie": serialize("ytp_tokens", tokens.accessToken, {
          "Max-Age": tokens.expiresIn,
          HttpOnly: true,
          Path: "/",
          SameSite: "Strict",
          Secure: true,
        }),
      },
    });
  }

  const state = crypto.randomUUID();
  const params = new URLSearchParams();

  params.append("redirect_uri", `${url.origin}/login`);
  params.append("state", state);
  params.append("client_id", process.env.GAUTH_CLIENT_ID!);
  params.append("response_type", "code");
  params.append("scope", "openid https://www.googleapis.com/auth/youtube");
  params.append("include_granted_scopes", "true");
  params.append("access_type", "offline");

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
