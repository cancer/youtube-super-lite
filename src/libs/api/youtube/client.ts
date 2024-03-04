import { Subscription } from "~/libs/api/youtube/types";

export type YouTubeApiClient = {
  request: <T = unknown>(args: {
    uri: string;
    method: "GET" | "POST";
    params?: Record<string, unknown>;
    body?: Record<string, unknown>;
  }) => Promise<T>;
};
export const createYouTubeApiClient: (args: {
  accessToken: string;
}) => YouTubeApiClient = ({ accessToken }) => ({
  request: async ({ uri, method, params, body }) => {
    const url = new URL(`https://youtube.googleapis.com/youtube/v3${uri}`);
    Object.entries(params ?? {}).forEach(([key, value]) =>
      url.searchParams.set(key, String(value)),
    );
    const res = await fetch(url, {
      method,
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${accessToken}`,
        Accept: "application/json",
      },
      body: body ? JSON.stringify(body) : undefined,
    });

    if (!res.ok) throw new Error(await res.text());

    return res.json();
  },
});

type PageInfo = { pageInfo: { totalResults: number; resultsPerPage: number } };

export type SubscriptionsRequest = {
  GET: { part: string[]; maxResults: number };
};
type SubscriptionsResponse = {
  GET: PageInfo & {
    items: Subscription[];
  };
};
export const listMyChannels =
  (client: YouTubeApiClient) =>
  ({
    part,
    maxResults
  }: SubscriptionsRequest["GET"]): Promise<SubscriptionsResponse["GET"]> => {
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
  GET: { id: string; part: string[]; }
};
export type ChannelResponse = {
  GET: {  }
}
