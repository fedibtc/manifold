// crates/fman/core/src/support.rs MAX_SUPPORT_MESSAGE_CHARS. The daemon
// refuses longer text; this only says so earlier.
export const MAX_SUPPORT_MESSAGE_CHARS = 4000;

// Characters as the daemon counts them (Rust `chars()`), not UTF-16 units.
export const supportMessageLength = (body: string) => [...body.trim()].length;

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

// The time above a group of messages, shaped like the Fedi app's chat
// (`formatMessageItemTimestamp`): the time alone today, else the date too. In
// UTC like every other time in this app, so a test asserts the value rather
// than the runner's timezone.
export const formatSupportTimestamp = (unixSeconds: number, nowMs = Date.now()) => {
  const iso = new Date(unixSeconds * 1000).toISOString();
  const time = `${iso.slice(11, 16)} UTC`;
  if (iso.slice(0, 10) === new Date(nowMs).toISOString().slice(0, 10)) return time;
  return `${MONTHS[Number(iso.slice(5, 7)) - 1]} ${iso.slice(8, 10)}, ${time}`;
};
