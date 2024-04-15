import { TokenExpiredError } from "~/libs/api/youtube/errors";
import type {
  PageInfo,
  Subscription,
  VideoGetRatingResponse,
} from "~/libs/api/youtube/types";
import {
  type AuthTokensClient,
  createAuthTokensClient,
} from "~/libs/auth-tokens/client";
import { getSession } from "~/libs/session";

type ApiClient = {
  request: <T = unknown>(args: {
    uri: string;
    method: "GET" | "POST";
    params?: Record<string, unknown>;
    body?: Record<string, unknown>;
  }) => Promise<T>;
};
const createApiClient = (authTokensClient: AuthTokensClient): ApiClient => {
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
    },
  };
};

// XXX: ほんとはこんなところでやりたくないが、コンポーネント経由で渡そうとするとシリアライズの問題が出るのでできない
let memo: ApiClient | null = null;
const client = () => {
  if (memo !== null) return memo;
  memo = createApiClient(
    createAuthTokensClient(() => getSession(process.env.SESSION_SECRET!)),
  );
  return memo;
};

export type MyChannelsRequest = {
  GET: { part: string[]; maxResults: number };
};
type MyChannelsResponse = {
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
  return client().request({
    uri: "/subscriptions",
    method: "GET",
    params,
  });
};

type VideoRatingRequest = {
  GET: { id: string };
};
type VideoRatingResponse = {
  GET: { rating: string };
};
export const getVideoRating = async ({
  id,
}: VideoRatingRequest["GET"]): Promise<VideoRatingResponse["GET"]> => {
  "use server";
  const params = {
    id,
  };
  return client()
    .request<VideoGetRatingResponse>({
      uri: "/videos/getRating",
      method: "GET",
      params,
    })
    .then((res) => {
      if (res.items.length === 0) return { rating: "" };
      return { rating: res.items[0].rating };
    });
};
