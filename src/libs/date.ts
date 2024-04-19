export const startOfDay = (date: Date | number) =>
  new Date(new Date(date).setHours(0, 0, 0, 0));

export const subtractDays = (date: Date | number, days: number) =>
  new Date(new Date(date).getTime() - days * 24 * 60 * 60 * 1000);
