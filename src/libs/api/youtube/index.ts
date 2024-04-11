import type {
  Subscription,
  VideoGetRatingResponse,
} from "~/libs/api/youtube/types";
import { type AuthTokens } from "~/libs/auth-tokens/types";
import { getAuthTokens } from "~/libs/session";

export class TokenExpiredError extends Error {
  name = "TokenExpiredError";

  constructor() {
    super("Token has expired.");
  }
}

export const isTokenExpired = (err: unknown): err is TokenExpiredError => {
  if (!err) return false;
  if (!(err instanceof TokenExpiredError)) return false;
  if (err.name !== "TokenExpiredError") return false;
  return true;
};
type PageInfo = { pageInfo: { totalResults: number; resultsPerPage: number } };
export type MyChannelsRequest = {
  GET: { part: string[]; maxResults: number };
};
export type MyChannelsResponse = {
  GET: PageInfo & {
    items: Subscription[];
  };
};
export const listMyChannels = ({
  part,
  maxResults,
}: MyChannelsRequest["GET"]): Promise<MyChannelsResponse["GET"]> => {
  "use server";
  const params = {
    maxResults,
    part: part.join(","),
    mine: true,
  };
  return request(() => getAuthTokens({ secret: process.env.SESSION_SECRET! }))({
    uri: "/subscriptions",
    method: "GET",
    params,
  });
};
export type VideoRatingRequest = {
  GET: { id: string };
};
export type VideoRatingResponse = {
  GET: { rating: string };
};
export const getVideoRating = async ({
  id,
}: VideoRatingRequest["GET"]): Promise<VideoRatingResponse["GET"]> => {
  "use server";
  const params = {
    id,
  };
  return request(() =>
    getAuthTokens({ secret: process.env.SESSION_SECRET! }),
  )<VideoGetRatingResponse>({
    uri: "/videos/getRating",
    method: "GET",
    params,
  }).then((res) => {
    if (res.items.length === 0) return { rating: "" };
    return { rating: res.items[0].rating };
  });
};
export const request = (
  getAuthTokens: () => Promise<AuthTokens | null>,
): (<T = unknown>(args: {
  uri: string;
  method: "GET" | "POST";
  params?: Record<string, unknown>;
  body?: Record<string, unknown>;
}) => Promise<T>) => {
  return async ({ uri, method, params, body }) => {
    let tokens;
    try {
      tokens = await getAuthTokens();
    } catch (err) {
      console.error("Failed to load tokens from session.", err);
      throw new TokenExpiredError();
    }
    if (tokens === null || Date.now() > tokens.expiresAt) {
      console.error("Retrieved tokens have expired.");
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

    if (!res.ok) throw new Error(await res.text());

    return res.json();
  };
};
