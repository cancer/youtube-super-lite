import { type RequestMiddleware } from "@solidjs/start/middleware";
import { getRequestEvent } from "solid-js/web";
import { createAuthApiClient, refreshAccessToken } from "~/libs/api/auth";
import { type ApiClient, createApiClient } from "~/libs/api/youtube/client";
import { TokenExpiredError } from "~/libs/api/youtube/errors";
import { AuthSession } from "~/libs/auth-sessions/client";

declare global {
  interface RequestLocals {
    youtubeApi: ApiClient;
  }
}

export const youtubeApi: () => RequestMiddleware = () => async (event) => {
  const { auth, env } = event.locals;
  const authApiClient = createAuthApiClient({
    clientId: env.GAUTH_CLIENT_ID,
    clientSecret: env.GAUTH_CLIENT_SECRET,
  });
  event.locals.youtubeApi = await createApiClient({
    async authenticate() {
      "use server";
      
      let tokens: AuthSession | null;
      try {
        tokens = await auth.get();
      } catch (err) {
        console.error("Failed to load tokens.", err);
        throw new TokenExpiredError();
      }
      if (tokens === null) {
        console.error("Could not retrieve tokens.");
        throw new TokenExpiredError();
      }
      if (Date.now() > tokens.expiresAt) {
        console.error("Retrieved tokens have expired.");

        try {
          const refreshed = await refreshAccessToken(authApiClient)(
            tokens.refreshToken,
          );
          tokens = {
            accessToken: refreshed.accessToken,
            refreshToken: tokens.refreshToken,
            expiresAt: Date.now() + refreshed.expiresIn * 1000,
          };
          await auth.set(tokens);
        } catch (err) {
          console.error("Failed to refresh tokens.", err);
          throw new TokenExpiredError();
        }
      }

      return tokens;
    },
  });
};
