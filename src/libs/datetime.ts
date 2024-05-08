const DAY = 24 * 60 * 60 * 1000;
const HOUR = 60 * 60 * 1000;

export const startOfDay = (date: Date | number) =>
  new Date(new Date(date).setHours(0, 0, 0, 0));

export const subtractDays = (date: Date | number, days: number) =>
  new Date(new Date(date).getTime() - days * DAY);

export const getHourDiff = (a: Date | number, b: Date | number) =>
  Math.floor((new Date(a).getTime() - new Date(b).getTime()) / HOUR);

// 現状アーカイブ可能なライブ配信は12時間未満なので、24時間を超える場合は考慮しない
// 少数点以下の値も考慮しない
const durationReg =
  /^([+\u2212-])?P(?:T(?!$)(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/i;
const toDurationTimeObject = (
  isoString: string,
): { hours: number; minutes: number; seconds: number } => {
  const match = durationReg.exec(isoString);
  if (!match) return { hours: 0, minutes: 0, seconds: 0 };

  const [_, signStr, hours, minutes, seconds] = match;
  const sign = signStr === "-" ? -1 : 1;
  return {
    hours: hours === undefined ? 0 : Number(hours) * sign,
    minutes: minutes === undefined ? 0 : Number(minutes) * sign,
    seconds: seconds === undefined ? 0 : Number(minutes) * sign,
  };
};
export const formatDurationTime = (isoString: string): string => {
  const { hours, minutes, seconds } = toDurationTimeObject(isoString);
  const paddedMinutes = String(minutes).padStart(2, "0");
  const paddedSeconds = String(seconds).padStart(2, "0");

  if (hours === 0) return `${minutes}:${paddedSeconds}`;
  return `${hours}:${paddedMinutes}:${paddedSeconds}`;
};
