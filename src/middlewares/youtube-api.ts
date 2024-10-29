import { type RequestMiddleware } from "@solidjs/start/middleware";
import { createAuthApiClient, refreshAccessToken } from "~/libs/api/auth";
import { type ApiClient, createApiClient } from "~/libs/api/youtube/client";
import { TokenExpiredError } from "~/libs/api/youtube/errors";
import { type AuthSession } from "~/libs/auth-sessions/client";

declare global {
  interface RequestEventLocals {
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

      let session: AuthSession | null;
      try {
        session = await auth.get();
      } catch (err) {
        console.error("Failed to load session.", err);
        throw new TokenExpiredError();
      }
      if (session === null) {
        console.error("Session not found.");
        throw new TokenExpiredError();
      }
      if (Date.now() > session.expiresAt) {
        console.error("Session has expired.");

        try {
          const refreshed = await refreshAccessToken(authApiClient)(
            session.refreshToken,
          );
          session = {
            accessToken: refreshed.accessToken,
            refreshToken: session.refreshToken,
            expiresAt: Date.now() + refreshed.expiresIn * 1000,
          };
          await auth.set(session);
        } catch (err) {
          console.error("Failed to refresh tokens.", err);
          await auth.clear();
          throw new TokenExpiredError();
        }
      }

      return session;
    },
  });
};
