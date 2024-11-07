// https://developers.google.com/youtube/v3/docs/videos/getRating?hl=ja
export type VideoGetRatingResponse = {
  kind: "youtube#videoGetRatingResponse";
  etag: string;
  items: {
    videoId: string;
    rating: string;
  }[];
};

