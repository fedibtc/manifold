// crates/fman/nostr/src/support.rs MAX_SUPPORT_MESSAGE_CHARS. The daemon
// refuses longer text; this only says so earlier.
export const MAX_SUPPORT_MESSAGE_CHARS = 4000;

// Characters as the daemon counts them (Rust `chars()`), not UTF-16 units.
export const supportMessageLength = (body: string) => [...body.trim()].length;

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

const localDate = (unixSeconds: number) => new Date(unixSeconds * 1000);

// The time in a bubble, in local time as the Fedi app's chat writes it
// (date-fns `h:mmaaa`).
export const formatSupportTime = (unixSeconds: number) => {
  const date = localDate(unixSeconds);
  const hours = date.getHours();
  return `${hours % 12 || 12}:${String(date.getMinutes()).padStart(2, '0')}${hours < 12 ? 'am' : 'pm'}`;
};

// The local day a message falls on, as the line above that day's messages.
export const formatSupportDay = (unixSeconds: number, now = new Date()) => {
  const date = localDate(unixSeconds);
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  if (date.toDateString() === now.toDateString()) return 'Today';
  if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
  const day = `${MONTHS[date.getMonth()]} ${date.getDate()}`;
  return date.getFullYear() === now.getFullYear() ? day : `${day}, ${date.getFullYear()}`;
};

export const isSameSupportDay = (a: number, b: number) =>
  localDate(a).toDateString() === localDate(b).toDateString();
