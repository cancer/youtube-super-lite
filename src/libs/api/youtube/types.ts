export type PageInfo = {
  pageInfo: { totalResults: number; resultsPerPage: number };
  nextPageToken: string;
  prevPageToken: string;
};

export type ListResponse<T> = PageInfo & {
  etag: string;
  items: T[];
};

// https://developers.google.com/youtube/v3/docs/subscriptions?hl=ja
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

// https://developers.google.com/youtube/v3/docs/channels?hl=ja
export type Channel = {
  kind: string;
  etag: string;
  id: string;
  snippet: {
    title: string;
    description: string;
    customUrl: string;
    publishedAt: string; // ISO8601 datetime string expected
    thumbnails: {
      [key: string]: {
        url: string;
        width: number; // unsigned integer
        height: number; // unsigned integer
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
    viewCount: number; // unsigned long
    subscriberCount: number; // unsigned long
    hiddenSubscriberCount: boolean;
    videoCount: number; // unsigned long
  };
  topicDetails: {
    topicIds: string[];
    topicCategories: string[];
  };
  status: {
    privacyStatus: string;
    isLinked: boolean;
    longUploadsStatus: string;
    madeForKids: boolean;
    selfDeclaredMadeForKids: boolean;
  };
  brandingSettings: {
    channel: {
      title: string;
      description: string;
      keywords: string;
      trackingAnalyticsAccountId: string;
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
    timeLinked: string; // ISO8601 datetime string expected
  };
  localizations: {
    [key: string]: {
      title: string;
      description: string;
    };
  };
};

// https://developers.google.com/youtube/v3/docs/search?hl=ja
export type SearchResult = {
  kind: "youtube#searchResult";
  string: string;
  id: {
    kind: string;
    videoId: string;
    channelId: string;
    playlistId: string;
  };
  snippet: {
    publishedAt: string;
    channelId: string;
    title: string;
    description: string;
    thumbnails: {
      [key: string]: {
        url: string;
        width: number;
        height: number;
      };
    };
    channelTitle: string;
    liveBroadcastContent: string;
  };
};

// https://developers.google.com/youtube/v3/docs/videos/getRating?hl=ja
export type VideoGetRatingResponse = {
  kind: "youtube#videoGetRatingResponse";
  etag: string;
  items: {
    videoId: string;
    rating: string;
  }[];
};

// https://developers.google.com/youtube/v3/docs/videos?hl=ja
export type Video = {
  kind: string;
  etag: string;
  id: string;
  snippet: {
    publishedAt: string; // ISO8601 datetime string expected
    channelId: string;
    title: string;
    description: string;
    thumbnails: {
      [key: string]: {
        url: string;
        width: number; // unsigned integer
        height: number; // unsigned integer
      };
    };
    channelTitle: string;
    tags: string[];
    categoryId: string;
    liveBroadcastContent: string;
    defaultLanguage: string;
    localized: {
      title: string;
      description: string;
    };
    defaultAudioLanguage: string;
  };
  contentDetails: {
    duration: string;
    dimension: string;
    definition: string;
    caption: string;
    licensedContent: boolean;
    regionRestriction: {
      allowed: string[];
      blocked: string[];
    };
    contentRating: {
      [rating: string]: string;
    };
    projection: string;
    hasCustomThumbnail: boolean;
  };
  status: {
    uploadStatus: string;
    failureReason: string;
    rejectionReason: string;
    privacyStatus: string;
    publishAt: string;
    license: string;
    embeddable: boolean;
    publicStatsViewable: boolean;
    madeForKids: boolean;
    selfDeclaredMadeForKids: boolean;
  };
  statistics: {
    viewCount: string;
    likeCount: string;
    dislikeCount: string;
    favoriteCount: string;
    commentCount: string;
  };
  player: {
    embedHtml: string;
    embedHeight: number; // long
    embedWidth: number; // long
  };
  topicDetails: {
    topicIds: string[];
    relevantTopicIds: string[];
    topicCategories: string[];
  };
  recordingDetails: {
    recordingDate: string;
  };
  fileDetails: {
    fileName: string;
    fileSize: number; // unsigned long
    fileType: string;
    container: string;
    videoStreams: {
      widthPixels: number; // unsigned integer
      heightPixels: number; // unsigned integer
      frameRateFps: number; // double
      aspectRatio: number; // double
      codec: string;
      bitrateBps: number; // unsigned long
      rotation: string;
      vendor: string;
    }[];
    audioStreams: {
      channelCount: number; // unsigned integer
      codec: string;
      bitrateBps: number; // unsigned long
      vendor: string;
    }[];
    durationMs: number; // unsigned long
    bitrateBps: number; // unsigned long
    creationTime: string;
  };
  processingDetails: {
    processingStatus: string;
    processingProgress: {
      partsTotal: number; // unsigned long
      partsProcessed: number; // unsigned long
      timeLeftMs: number; // unsigned long
    };
    processingFailureReason: string;
    fileDetailsAvailability: string;
    processingIssuesAvailability: string;
    tagSuggestionsAvailability: string;
    editorSuggestionsAvailability: string;
    thumbnailsAvailability: string;
  };
  suggestions: {
    processingErrors: string[];
    processingWarnings: string[];
    processingHints: string[];
    tagSuggestions: {
      tag: string;
      categoryRestricts: string[];
    }[];
    editorSuggestions: string[];
  };
  liveStreamingDetails: {
    actualStartTime: string;
    actualEndTime: string;
    scheduledStartTime: string;
    scheduledEndTime: string;
    concurrentViewers: number; // unsigned long
    activeLiveChatId: string;
  };
  localizations: {
    [key: string]: {
      title: string;
      description: string;
    };
  };
};

export type PlaylistItem = {
  kind: string;
  etag: string;
  id: string;
  snippet: {
    publishedAt: string; // ISO8601 datetime string expected
    channelId: string;
    title: string;
    description: string;
    thumbnails: {
      [key: string]: {
        url: string;
        width: number; // unsigned integer
        height: number; // unsigned integer
      };
    };
    channelTitle: string;
    videoOwnerChannelTitle: string;
    videoOwnerChannelId: string;
    playlistId: string;
    position: number; // unsigned integer
    resourceId: {
      kind: string;
      videoId: string;
    };
  };
  contentDetails: {
    videoId: string;
    startAt: string;
    endAt: string;
    note: string;
    videoPublishedAt: string; // ISO8601 datetime string expected
  };
  status: {
    privacyStatus: string;
  };
};

// https://developers.google.com/youtube/v3/docs/activities?hl=ja
export type Activity = {
  kind: string;
  etag: string;
  id: string;
  snippet: {
    publishedAt: string; // ISO8601 datetime string expected
    channelId: string;
    title: string;
    description: string;
    thumbnails: {
      [key: string]: {
        url: string;
        width: number; // unsigned integer
        height: number; // unsigned integer
      };
    };
    channelTitle: string;
    type: string;
    groupId: string;
  };
  contentDetails: {
    upload?: {
      videoId: string;
    };
    like?: {
      resourceId: {
        kind: string;
        videoId: string;
      };
    };
    favorite?: {
      resourceId: {
        kind: string;
        videoId: string;
      };
    };
    comment?: {
      resourceId: {
        kind: string;
        videoId: string;
        channelId: string;
      };
    };
    subscription?: {
      resourceId: {
        kind: string;
        channelId: string;
      };
    };
    playlistItem?: {
      resourceId: {
        kind: string;
        videoId: string;
      };
      playlistId: string;
      playlistItemId: string;
    };
    recommendation?: {
      resourceId: {
        kind: string;
        videoId: string;
        channelId: string;
      };
      reason: string;
      seedResourceId: {
        kind: string;
        videoId: string;
        channelId: string;
        playlistId: string;
      };
    };
    social?: {
      type: string;
      resourceId: {
        kind: string;
        videoId: string;
        channelId: string;
        playlistId: string;
      };
      author: string;
      referenceUrl: string;
      imageUrl: string;
    };
    channelItem?: {
      resourceId: {};
    };
  };
};
