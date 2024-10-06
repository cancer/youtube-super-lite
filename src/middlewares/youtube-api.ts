import { type RequestMiddleware } from "@solidjs/start/middleware";
import { getRequestEvent } from "solid-js/web";
import { type ApiClient, createApiClient } from "~/libs/api/youtube/client";

declare global {
  interface RequestLocals {
    youtubeApi: ApiClient;
  }
}

export const youtubeApi: () => RequestMiddleware = () => async (event) => {
  const ev = getRequestEvent()!;
  event.locals.youtubeApi = createApiClient({
    getTokens() {
      return ev.locals.auth.get();
    },
    async revokeTokens() {
      await ev.locals.auth.clear();
    },
  });
};
