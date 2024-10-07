import { type RequestMiddleware } from "@solidjs/start/middleware";
import { getRequestEvent } from "solid-js/web";
import { refreshAccessToken } from "~/libs/api/auth";
import { type ApiClient, createApiClient } from "~/libs/api/youtube/client";

declare global {
  interface RequestLocals {
    youtubeApi: ApiClient;
  }
}

export const youtubeApi: () => RequestMiddleware = () => async (event) => {
  const { auth, authClient } = getRequestEvent()!.locals;
  event.locals.youtubeApi = createApiClient({
    async getTokens() {
      "use server";
      return auth.get();
    },
    async refreshTokens(_refreshToken) {
      "use server";
      const { accessToken, refreshToken, expiresIn } =
        await refreshAccessToken(authClient)(_refreshToken);
      const tokens = {
        accessToken,
        refreshToken,
        expiresAt: Date.now() + expiresIn * 1000,
      };
      await auth.set(tokens);
      return tokens;
    },
  });
};
