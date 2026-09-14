import { bech32, bech32m } from '@scure/base';
import { normalizeFieldText } from '@/shared/utils/fieldText';

const LIGHTNING_SCHEME = /^lightning:/i;

/**
 * The destination as the daemon should store it. A copied or scanned payment
 * link can carry a `lightning:` scheme that the daemon's parser refuses. LUD-16
 * allows only lowercase in a Lightning address, and an LNURL decodes the same
 * in either case, so lowercase suits both.
 */
export const normalizePayoutDestination = (input: string): string =>
  normalizeFieldText(input).replace(LIGHTNING_SCHEME, '').toLowerCase();

/** LUD-16 username characters, then a domain. The daemon's address parser
 *  accepts a single-label domain, so this does not require a dot. */
const LIGHTNING_ADDRESS = /^[a-z0-9._+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)*$/;
/** The longest string the daemon's bech32 decoder reads (bech32 0.11 `CODE_LENGTH`). */
const BECH32_MAX_LENGTH = 1023;
const UTF8 = new TextDecoder('utf-8', { fatal: true });

/**
 * LUD-01: a bech32 string with the `lnurl` prefix whose data is the service URL.
 * Read the way the daemon reads it (lnurl-rs `LnUrl::from_str`): either checksum,
 * then UTF-8 text.
 */
const isLnurl = (destination: string): boolean => {
  const decoded =
    bech32.decodeUnsafe(destination, BECH32_MAX_LENGTH) ??
    bech32m.decodeUnsafe(destination, BECH32_MAX_LENGTH);
  if (decoded?.prefix !== 'lnurl') return false;
  const bytes = bech32.fromWordsUnsafe(decoded.words);
  if (!bytes) return false;
  try {
    UTF8.decode(bytes);
    return true;
  } catch {
    return false;
  }
};

/**
 * Whether a normalized destination parses the way the daemon parses it
 * (crates/fman/fedimint/src/lib.rs:1208). The daemon parses it only when a
 * payout starts, so without this a value that can never be paid is stored in
 * silence. A destination that parses can still fail when its service is asked
 * for an invoice, or name someone else's wallet.
 */
export const isPayoutDestinationFormat = (destination: string): boolean =>
  LIGHTNING_ADDRESS.test(destination) || isLnurl(destination);
