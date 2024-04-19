import { TokenExpiredError } from "~/libs/api/youtube/errors";
import type {
  Channel,
  ListResponse,
  PageInfo,
  PlaylistItem,
  Subscription,
  Video,
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

export type LatestVideoListRequestGet = {
  maxResults: number;
  publishedAfter: Date;
};
export type LatestVideoListResponseGet = ListResponse<
  Pick<Video, "id" | "snippet" | "contentDetails" | "liveStreamingDetails"> & {
    isShorts: boolean;
  }
>;
export const listLatestVideos = async ({
  maxResults,
  publishedAfter,
}: LatestVideoListRequestGet): Promise<LatestVideoListResponseGet> => {
  "use server";
  // まずはすべての登録チャンネルを取ってきて、
  const channels: ListResponse<Pick<Subscription, "snippet">> =
    await client().request({
      uri: "/subscriptions",
      method: "GET",
      params: {
        maxResults,
        part: "snippet",
        mine: true,
      },
    });

  // 最新の動画を集めてくる（ライブ含む）
  const channelIds = channels.items.map(
    ({
      snippet: {
        resourceId: { channelId },
      },
    }) => channelId,
  );
  const latestUploadPlaylists = await client()
    .request<ListResponse<Pick<Channel, "snippet" | "contentDetails">>>({
      uri: "/channels",
      method: "GET",
      params: {
        part: "snippet, contentDetails",
        id: channelIds.join(","),
      },
    })
    .then(({ items }) =>
      items.map(
        ({ contentDetails }) => contentDetails.relatedPlaylists.uploads,
      ),
    );
  const latestUploads = await Promise.all(
    latestUploadPlaylists.map((playlistId) =>
      client()
        .request<ListResponse<Pick<PlaylistItem, "contentDetails">>>({
          uri: "/playlistItems",
          method: "GET",
          params: {
            part: "contentDetails",
            playlistId,
          },
        })
        .then(({ items }) => items),
    ),
  );

  // アップロード日順にソートしたvideoIdを集めて
  const videoIds = latestUploads
    .flat()
    .sort((a, b) => {
      if (a.contentDetails.videoPublishedAt < b.contentDetails.videoPublishedAt)
        return 1;
      if (a.contentDetails.videoPublishedAt > b.contentDetails.videoPublishedAt)
        return -1;
      return 0;
    })
    .reduce(
      (acc: string[], { contentDetails: { videoId, videoPublishedAt } }) => {
        if (new Date(videoPublishedAt) < publishedAfter) return acc;
        acc.push(videoId);
        return acc;
      },
      [],
    )
    .slice(0, maxResults);

  // video詳細をリクエスト
  return await client().request<LatestVideoListResponseGet>({
    uri: "/videos",
    method: "GET",
    params: {
      part: "id,snippet, contentDetails, liveStreamingDetails",
      id: videoIds.join(","),
      maxResults,
    },
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
