// crates/fman/core/src/support.rs MAX_SUPPORT_MESSAGE_CHARS. The daemon
// refuses longer text; this only says so earlier.
export const MAX_SUPPORT_MESSAGE_CHARS = 4000;

// Characters as the daemon counts them (Rust `chars()`), not UTF-16 units.
export const supportMessageLength = (body: string) => [...body.trim()].length;

// Chat times render in UTC like every other time in this app, so a test asserts
// the value rather than the runner's timezone.
// Assembled from parts: ICU versions disagree on the commas of a full date.
export const formatSupportDay = (unixSeconds: number) => {
  const date = new Date(unixSeconds * 1000);
  const name = (options: Intl.DateTimeFormatOptions) =>
    date.toLocaleDateString('en-GB', { ...options, timeZone: 'UTC' });
  return `${name({ weekday: 'long' })} ${date.getUTCDate()} ${name({ month: 'long' })} ${date.getUTCFullYear()}`;
};

export const formatSupportTime = (unixSeconds: number) =>
  `${new Date(unixSeconds * 1000).toISOString().slice(11, 16)} UTC`;
