import type { Plan } from '@operator-ui/types';

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

/**
 * One field carries all three offer states, because the wire has exactly three:
 * blank is `null` (not selling), `0` is a free seat that is still advertised,
 * and anything else is the price the initiator pays.
 */
export const parsePriceField = (input: string): ParsedPrice => {
  const trimmed = input.trim();
  if (trimmed === '') return { ok: true, priceMsat: null };

  const sats = Number(trimmed);
  if (!Number.isFinite(sats)) return { ok: false, error: 'Enter a whole number of sats.' };
  if (!Number.isInteger(sats)) return { ok: false, error: 'Sats cannot be fractional.' };
  if (sats < 0) return { ok: false, error: 'A price cannot be negative.' };

  // The conversion is where precision is lost, so the bound is checked after it.
  // A msat value past Number.MAX_SAFE_INTEGER does not survive JSON: the daemon
  // would store a number the operator never typed.
  const priceMsat = sats * MSATS_PER_SAT;
  if (!Number.isSafeInteger(priceMsat)) return { ok: false, error: 'That price is too large.' };

  return { ok: true, priceMsat };
};

/** How the offer reads on a summary surface, in the operator's words. */
export const describeOffer = (priceMsat: number | null): string => {
  if (priceMsat === null) return 'Not selling seats';
  if (priceMsat === 0) return 'Free';
  return `${(priceMsat / MSATS_PER_SAT).toLocaleString('en-US')} sats per seat`;
};
