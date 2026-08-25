// Middle-truncate a long opaque identifier (pubkey, hash, endpoint) for display.
export const truncateMiddle = (value: string, head = 8, tail = 4): string =>
  value.length <= head + tail + 1 ? value : `${value.slice(0, head)}…${value.slice(-tail)}`;

// Whether truncateMiddle would shorten this value at the given head/tail —
// callers use this to only offer a copy affordance where text was actually cut.
export const isTruncated = (value: string, head = 8, tail = 4): boolean =>
  value.length > head + tail + 1;
