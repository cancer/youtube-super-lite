import type { ApiClient } from "~/libs/api/youtube/client";
import type { VideoGetRatingResponse } from "~/libs/api/youtube/types";

export type VideoRatingRequest = {
  GET: { id: string };
  POST: { id: string; rating: "like" };
};
export type VideoRatingResponse = {
  GET: { rating: string };
  POST: unknown;
};
export const getVideoRating = (client: ApiClient) => {
  "use server";
  return async ({
    id,
  }: VideoRatingRequest["GET"]): Promise<VideoRatingResponse["GET"]> => {
    "use server";
    const params = {
      id,
    };
    return client
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
};
export const postVideoRating = (client: ApiClient) => {
  "use server";
  return async (
    params: VideoRatingRequest["POST"],
  ): Promise<VideoRatingResponse["POST"]> => {
    "use server";

    return client.request<unknown>({
      uri: "/videos/rate",
      method: "POST",
      params,
    });
  };
};
