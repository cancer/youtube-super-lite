import { TokenExpiredError } from "~/libs/api/youtube/errors";
import { type AuthTokensClient } from "~/libs/auth-tokens/client";

export type ApiClient = {
  request: <T = unknown>(args: {
    uri: string;
    method: "GET" | "POST";
    params?: Record<string, unknown>;
    body?: Record<string, unknown>;
  }) => Promise<T>;
};

export const createApiClient = (authTokensClient: AuthTokensClient): ApiClient => {
  "use server";
  return {
    request: async ({ uri, method, params, body }) => {
      let tokens;
      try {
        tokens = await authTokensClient.get();
      } catch (err) {
        console.error("Failed to load tokens from session.", err);
        throw new TokenExpiredError();
      }
      if (tokens === null || Date.now() > tokens.expiresAt) {
        console.error("Retrieved tokens have expired.");
        throw new TokenExpiredError();
      }
      
      const url = new URL(`https://youtube.googleapis.com/youtube/v3${ uri }`);
      Object.entries(params ?? {}).forEach(([key, value]) =>
        url.searchParams.set(key, String(value)),
      );
      const res = await fetch(url, {
        method,
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${ tokens.accessToken }`,
          Accept: "application/json",
        },
        body: body ? JSON.stringify(body) : undefined,
      });
      
      if (!res.ok) {
        const json = (await res.json()) as any;
        if (
          res.status === 401 &&
          json.error.errors.some((e: any) => e.reason === "authError")
        ) {
          await authTokensClient.clear();
          throw new TokenExpiredError();
        }
        throw new Error(JSON.stringify(json));
      }
      
      return res.json();
    },
  };
};
