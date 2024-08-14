import { TokenExpiredError } from "~/libs/api/youtube/errors";
import { type AuthTokens } from "~/libs/auth-tokens/types";

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
  revokeTokens,
}: {
  getTokens: () => Promise<AuthTokens | null>;
  revokeTokens: () => Promise<void>;
}): ApiClient => {
  "use server";
  return {
    async request({ uri, method, params, body }) {
      let tokens;
      try {
        tokens = await getTokens();
      } catch (err) {
        console.error("Failed to load tokens from session.", err);
        await revokeTokens().catch(() =>
          console.error("Failed to revoke tokens."),
        );
        throw new TokenExpiredError();
      }
      if (tokens === null || Date.now() > tokens.expiresAt) {
        console.error("Retrieved tokens have expired.");
        await revokeTokens().catch(() =>
          console.error("Failed to revoke tokens."),
        );
        throw new TokenExpiredError();
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
            await revokeTokens().catch(() =>
              console.error("Failed to revoke tokens."),
            );
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
