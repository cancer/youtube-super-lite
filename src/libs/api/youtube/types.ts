export type Subscription = {
  kind: "youtube#subscription";
  string: string;
  id: string;
  snippet: {
    publishedAt: string;
    channelTitle: string;
    title: string;
    description: string;
    resourceId: {
      kind: string;
      channelId: string;
    };
    channelId: string;
    thumbnails: {
      [key: string]: {
        url: string;
        width: number;
        height: number;
      };
    };
  };
  contentDetails: {
    totalItemCount: number;
    newItemCount: number;
    activityType: string;
  };
  subscriberSnippet: {
    title: string;
    description: string;
    channelId: string;
    thumbnails: {
      [key: string]: {
        url: string;
        width: number;
        height: number;
      };
    };
  };
};

export type Channel = {
  kind: "youtube#channel";
  string: string;
  id: string;
  snippet: {
    title: string;
    description: string;
    customUrl: string;
    publishedAt: string;
    thumbnails: {
      [key: string]: {
        url: string;
        width: number;
        height: number;
      };
    };
    defaultLanguage: string;
    localized: {
      title: string;
      description: string;
    };
    country: string;
  };
  contentDetails: {
    relatedPlaylists: {
      likes: string;
      favorites: string;
      uploads: string;
    };
  };
  statistics: {
    viewCount: number;
    subscriberCount: number; // this value is rounded to three significant figures
    hiddenSubscriberCount: boolean;
    videoCount: number;
  };
  topicDetails: {
    topicIds: [string];
    topicCategories: [string];
  };
  status: {
    privacyStatus: string;
    isLinked: boolean;
    numberUploadsStatus: string;
    madeForKids: boolean;
    selfDeclaredMadeForKids: boolean;
  };
  brandingSettings: {
    channel: {
      title: string;
      description: string;
      keywords: string;
      trackingAnalyticsAccountId: string;
      moderateComments: boolean;
      unsubscribedTrailer: string;
      defaultLanguage: string;
      country: string;
    };
    watch: {
      textColor: string;
      backgroundColor: string;
      featuredPlaylistId: string;
    };
  };
  auditDetails: {
    overallGoodStanding: boolean;
    communityGuidelinesGoodStanding: boolean;
    copyrightStrikesGoodStanding: boolean;
    contentIdClaimsGoodStanding: boolean;
  };
  contentOwnerDetails: {
    contentOwner: string;
    timeLinked: string;
  };
  localizations: {
    [key: string]: {
      title: string;
      description: string;
    };
  };
};

export type SearchResult = {
  "kind": "youtube#searchResult",
  "string": string,
  "id": {
    "kind": string,
    "videoId": string,
    "channelId": string,
    "playlistId": string
  },
  "snippet": {
    "publishedAt": string,
    "channelId": string,
    "title": string,
    "description": string,
    "thumbnails": {
      [key: string]: {
        "url": string,
        "width": number,
        "height": number
      }
    },
    "channelTitle": string,
    "liveBroadcastContent": string
  }
};

export type VideoGetRatingResponse = {
  kind: "youtube#videoGetRatingResponse";
  etag: string;
  items: {
    videoId: string;
    rating: string;
  }[];
};
