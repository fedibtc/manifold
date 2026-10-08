// crates/fman/nostr/src/support.rs MAX_SUPPORT_MESSAGE_CHARS. The daemon
// refuses longer text; this only says so earlier.
export const MAX_SUPPORT_MESSAGE_CHARS = 4000;

// Characters as the daemon counts them (Rust `chars()`), not UTF-16 units.
export const supportMessageLength = (body: string) => [...body.trim()].length;

const WEEKDAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const MONTHS = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December'
];

const localDate = (unixSeconds: number) => new Date(unixSeconds * 1000);

const twoDigits = (value: number) => String(value).padStart(2, '0');

// The time in a bubble, in local 24-hour time as the operator design writes it.
export const formatSupportTime = (unixSeconds: number) => {
  const date = localDate(unixSeconds);
  return `${twoDigits(date.getHours())}:${twoDigits(date.getMinutes())}`;
};

// The local day a message falls on, as the line above that day's messages.
export const formatSupportDay = (unixSeconds: number, now = new Date()) => {
  const date = localDate(unixSeconds);
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  if (date.toDateString() === now.toDateString()) return 'Today';
  if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
  const day = `${WEEKDAYS[date.getDay()]}, ${date.getDate()} ${MONTHS[date.getMonth()]}`;
  return date.getFullYear() === now.getFullYear() ? day : `${day} ${date.getFullYear()}`;
};

export const isSameSupportDay = (a: number, b: number) =>
  localDate(a).toDateString() === localDate(b).toDateString();
