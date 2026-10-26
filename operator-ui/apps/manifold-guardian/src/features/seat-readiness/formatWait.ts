/** A wait the daemon reported, in words. */
export const formatWait = (secs: number): string => {
  const hours = secs / 3600;
  if (hours < 1) return 'less than an hour';
  if (hours < 36) return `about ${Math.round(hours)} ${Math.round(hours) === 1 ? 'hour' : 'hours'}`;
  return `about ${Math.round(hours / 24)} days`;
};
