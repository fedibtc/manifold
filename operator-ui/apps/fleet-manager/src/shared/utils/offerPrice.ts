import type { Plan } from '@operator-ui/types';
import { parseWholeNumberInput, type WholeNumberRejection } from '@/shared/utils/wholeNumberInput';

const MSATS_PER_SAT = 1000;

/** The stored price behind the offer, in millisatoshis. `null` is "not selling":
 *  the daemon renders no stored price as an empty plan list. */
export const readOfferPriceMsat = (plans: Plan[]): number | null => {
  const paid = plans.find(
    (plan): plan is { InfiniteBestEffort: { price_msats: number } } => 'InfiniteBestEffort' in plan
  );
  return paid ? paid.InfiniteBestEffort.price_msats : null;
};

/** What the price field shows for a stored price: sats, or blank when not selling. */
export const formatPriceField = (priceMsat: number | null): string =>
  priceMsat === null ? '' : String(priceMsat / MSATS_PER_SAT);

export type ParsedPrice = { ok: true; priceMsat: number | null } | { ok: false; error: string };

const PRICE_REJECTIONS: Record<Exclude<WholeNumberRejection, 'blank'>, string> = {
  'not-a-number': 'Enter a whole number of sats.',
  fractional: 'Sats cannot be fractional.',
  negative: 'A price cannot be negative.',
  'too-large': 'That price is too large.'
};

/**
 * One field carries all three offer states, because the wire has exactly three:
 * blank is `null` (not selling), `0` is a free seat that is still advertised,
 * and anything else is the price the initiator pays. A grouped price reads as
 * the number it shows, so the form `describeOffer` prints can be typed back.
 */
export const parsePriceField = (input: string): ParsedPrice => {
  const parsed = parseWholeNumberInput(input);
  if (!parsed.ok) {
    return parsed.reason === 'blank'
      ? { ok: true, priceMsat: null }
      : { ok: false, error: PRICE_REJECTIONS[parsed.reason] };
  }

  // The conversion is where precision is lost, so the bound is checked after it.
  // A msat value past Number.MAX_SAFE_INTEGER does not survive JSON: the daemon
  // would store a number the operator never typed.
  const priceMsat = parsed.value * MSATS_PER_SAT;
  if (!Number.isSafeInteger(priceMsat)) {
    return { ok: false, error: PRICE_REJECTIONS['too-large'] };
  }

  return { ok: true, priceMsat };
};

/** How the offer reads on a summary surface, in the operator's words. */
export const describeOffer = (priceMsat: number | null): string => {
  if (priceMsat === null) return 'Not selling seats';
  if (priceMsat === 0) return 'Free';
  return `${(priceMsat / MSATS_PER_SAT).toLocaleString('en-US')} sats per seat`;
};
