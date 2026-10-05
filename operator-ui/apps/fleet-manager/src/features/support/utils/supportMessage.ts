// crates/fman/core/src/support.rs MAX_SUPPORT_MESSAGE_CHARS. The daemon
// refuses longer text; this only says so earlier.
export const MAX_SUPPORT_MESSAGE_CHARS = 4000;

// Characters as the daemon counts them (Rust `chars()`), not UTF-16 units.
export const supportMessageLength = (body: string) => [...body.trim()].length;

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

// The time above a group of messages, as the Fedi app's chat shows it
// (`formatMessageItemTimestamp`, date-fns `h:mmaaa` and `MMM dd, h:mmaaa`):
// local time, with the date only before today.
export const formatSupportTimestamp = (unixSeconds: number, now = new Date()) => {
  const date = new Date(unixSeconds * 1000);
  const hours = date.getHours();
  const time = `${hours % 12 || 12}:${String(date.getMinutes()).padStart(2, '0')}${hours < 12 ? 'am' : 'pm'}`;
  if (date.toDateString() === now.toDateString()) return time;
  return `${MONTHS[date.getMonth()]} ${String(date.getDate()).padStart(2, '0')}, ${time}`;
};
