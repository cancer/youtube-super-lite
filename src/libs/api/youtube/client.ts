import type {
  Channel,
  SearchResult,
  Subscription,
} from "~/libs/api/youtube/types";
import { AuthTokens, getAuthTokens } from "~/libs/session";

export class TokenExpiredError extends Error {
  name = "TokenExpiredError";
  constructor() {
    super("Token has expired.");
  }
}

export type YouTubeApiClient = {
  request: <T = unknown>(args: {
    uri: string;
    method: "GET" | "POST";
    params?: Record<string, unknown>;
    body?: Record<string, unknown>;
  }) => Promise<T>;
};
export const createYouTubeApiClient: (args: {
  getAuthTokens: () => Promise<AuthTokens | null>;
}) => YouTubeApiClient = ({ getAuthTokens }) => ({
  request: async ({ uri, method, params, body }) => {
    "use server";
    let tokens;
    try {
      tokens = await getAuthTokens();
    } catch (e) {
      console.log(e);
      throw new TokenExpiredError();
    }
    if (tokens === null || Date.now() > tokens.expiresAt)
      throw new TokenExpiredError();

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
});

type PageInfo = { pageInfo: { totalResults: number; resultsPerPage: number } };

export type MyChannelsRequest = {
  GET: { part: string[]; maxResults: number };
};
export type MyChannelsResponse = {
  GET: PageInfo & {
    items: Subscription[];
  };
};
export const listMyChannels =
  (client: YouTubeApiClient) =>
  ({
    part,
    maxResults,
  }: MyChannelsRequest["GET"]): Promise<MyChannelsResponse["GET"]> => {
    const params = {
      maxResults,
      part: part.join(","),
      mine: true,
    };
    return client.request({
      uri: "/subscriptions",
      method: "GET",
      params,
    });
  };

export type ChannelRequest = {
  GET: { id: string; part: string[] };
};
export type ChannelResponse = {
  GET: Channel;
};
export const getChannel =
  (client: YouTubeApiClient) =>
  async ({
    id,
    part,
  }: ChannelRequest["GET"]): Promise<ChannelResponse["GET"]> => {
    const params = {
      id,
      part: part.join(","),
    };
    const result = await client.request<{ items: Channel[] }>({
      uri: "/channels",
      method: "GET",
      params,
    });

    return result.items[0];
  };

export type VideosRequest = {
  GET: { channelId: string; maxResults: number; order: string; part: string[] };
};
export type VideosResponse = {
  GET: PageInfo & { items: SearchResult[] };
};
export const listVideos =
  (client: YouTubeApiClient) =>
  async ({
    channelId,
    maxResults,
    order,
    part,
  }: VideosRequest["GET"]): Promise<VideosResponse["GET"]> => {
    const params = {
      channelId,
      maxResults,
      order,
      part: part.join(","),
      type: "video",
    };
    return client.request({
      uri: "/search",
      method: "GET",
      params,
    });
  };
