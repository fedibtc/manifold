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
/** LUD-01: the `lnurl` prefix, the bech32 separator, then bech32 characters. */
const LNURL = /^lnurl1[02-9ac-hj-np-z]+$/;

/**
 * Whether a normalized destination has one of the two shapes the daemon can pay
 * (crates/fman/fedimint/src/lib.rs:1208). The daemon parses it only when a
 * payout starts, so without this a value that can never be paid is stored in
 * silence. It checks the shape only: a well-formed address can still name
 * someone else's wallet.
 */
export const isPayoutDestinationFormat = (destination: string): boolean =>
  LIGHTNING_ADDRESS.test(destination) || LNURL.test(destination);
