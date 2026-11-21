// Wire indices are append-only and mirror server ClientMetrics.
export const metric = {
  uploadPrepare: 0,
  uploadAllocate: 1,
  uploadPut: 2,
  uploadVerify: 3,
  uploadReady: 4,
  uploadFailedPrepare: 5,
  uploadFailedAllocate: 6,
  uploadFailedPut: 7,
  uploadFailedVerify: 8,
  uploadCancelled: 9,
  uploadSmall: 10,
  uploadMedium: 11,
  uploadLarge: 12,
  chatStartup: 13,
  chatReconnect: 14,
  chatHandshake: 15,
  chatSync: 16,
  messageAck: 17,
  messageFailed: 18,
  messageInterrupted: 19,
  historyReused: 20,
  historyFetched: 21,
  mediaBytesReused: 22,
  mediaBytesFetched: 23,
  mediaLeaseReused: 24,
  mediaLeaseResolved: 25,
  mediaConfigReused: 26,
  mediaConfigJoined: 27,
  mediaConfigFetched: 28,
  mediaLeaseJoined: 29,
  mediaPreview: 30,
  mediaPreviewFailed: 31,
  historyFailed: 32,
  mediaStatusReused: 33,
  mediaStatusJoined: 34,
  mediaStatusFetched: 35,
  libraryJoined: 36,
  libraryFetched: 37,
  firstChat: 38,
  firstMedia: 39,
  firstSticker: 40,
  firstSend: 41,
  firstUpload: 42,
  renderError: 43,
  scriptError: 44,
  promiseError: 45,
  bootstrapError: 46,
  apiNetworkError: 47,
  apiTimeout: 48,
  apiInvalidResponse: 49,
  socketFailed: 50,
  socketDisconnected: 51,
  socketResumed: 52,
  socketResynced: 53,
  mediaWorkerFailed: 54,
  stickerFailed: 55,
  playbackFailed: 56,
  mediaWorkerReady: 57,
  firstSignedInChat: 58,
  apiRejected: 59,
  apiServerError: 60,
  socketInvalid: 61,
  mediaPlaybackReady: 62,
  uploadRejected: 63,
  browserResizeNotice: 64,
} as const;

export type ClientMetric = (typeof metric)[keyof typeof metric];
export const mediaMetrics = [
  metric.mediaBytesReused,
  metric.mediaBytesFetched,
  metric.mediaLeaseReused,
  metric.mediaLeaseResolved,
  metric.mediaLeaseJoined,
] as const;
export type MediaMetric = (typeof mediaMetrics)[number];
export const mediaBucketCounts = [8, 8, 1, 1, 1] as const;
export const maxMetricCount = 256;

export const durationBucketCount = 8;
const limits = [10, 50, 100, 250, 1000, 5000, 30000];

export function durationBucket(elapsed: number): number {
  let bucket = 0;
  while (bucket < limits.length && elapsed > (limits[bucket] ?? Infinity))
    bucket++;
  return bucket;
}
