import { TokenExpiredError } from "~/libs/api/youtube/errors";
import type { AuthSession } from "~/libs/auth-sessions/client";

export type ApiClient = {
  request: <T = unknown>(args: {
    uri: string;
    method: "GET" | "POST";
    params?: Record<string, unknown>;
    body?: Record<string, unknown>;
  }) => Promise<T>;
};

export const createApiClient = ({
  getTokens,
  refreshTokens,
}: {
  getTokens: () => Promise<AuthSession | null>;
  refreshTokens: (refreshToken: string) => Promise<AuthSession>;
}): ApiClient => {
  return {
    async request({ uri, method, params, body }) {
      "use server";
      let tokens;
      try {
        tokens = await getTokens();
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
          tokens = await refreshTokens(tokens.refreshToken);
        } catch (err) {
          console.error("Failed to refresh tokens.", err);
          throw new TokenExpiredError();
        }
      }

      const url = new URL(`https://youtube.googleapis.com/youtube/v3${uri}`);
      Object.entries(params ?? {}).forEach(([key, value]) =>
        url.searchParams.set(key, String(value)),
      );
      const res = await fetch(url, {
        method,
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${tokens.accessToken}`,
          Accept: "application/json",
        },
        body: body ? JSON.stringify(body) : undefined,
      });

      if (!res.ok) {
        if (res.headers.get("Content-Type") === "application/json") {
          const json = (await res.json()) as any;
          if (
            res.status === 401 &&
            json.error.errors.some((e: any) => e.reason === "authError")
          ) {
            console.error("Failed to authenticate request.", json);
            throw new TokenExpiredError();
          }
          throw new Error(JSON.stringify(json));
        }

        throw new Error(`Fetch failed. Status: ${res.status}`);
      }

      if (res.status === 200) return res.json();
      return {} as any;
    },
  };
};
