import type { Plan } from '@operator-ui/types';
import {
  describeOffer,
  formatPriceField,
  parsePriceField,
  readOfferPriceMsat
} from '../offerPrice';

it('should read no price from an empty plan list', () => {
  expect(readOfferPriceMsat([])).toBe(null);
});

it('should read the price out of the offered plan', () => {
  const plans: Plan[] = [{ InfiniteBestEffort: { price_msats: 50_000_000 } }];

  expect(readOfferPriceMsat(plans)).toBe(50_000_000);
});

it('should read a zero price as free rather than as not selling', () => {
  const plans: Plan[] = [{ InfiniteBestEffort: { price_msats: 0 } }];

  expect(readOfferPriceMsat(plans)).toBe(0);
});

it('should show a blank field when there is no stored price', () => {
  expect(formatPriceField(null)).toBe('');
});

it('should show the stored price in sats', () => {
  expect(formatPriceField(50_000_000)).toBe('50000');
});

it('should parse a blank field as not selling', () => {
  expect(parsePriceField('   ')).toEqual({ ok: true, priceMsat: null });
});

it('should parse zero as a free seat', () => {
  expect(parsePriceField('0')).toEqual({ ok: true, priceMsat: 0 });
});

it('should convert sats to millisatoshis', () => {
  expect(parsePriceField('50000')).toEqual({ ok: true, priceMsat: 50_000_000 });
});

it('should reject a non-numeric price', () => {
  expect(parsePriceField('lots')).toEqual({
    ok: false,
    error: 'Enter a whole number of sats.'
  });
});

it('should reject a fractional price', () => {
  expect(parsePriceField('12.5')).toEqual({ ok: false, error: 'Sats cannot be fractional.' });
});

it('should reject a negative price', () => {
  expect(parsePriceField('-1')).toEqual({ ok: false, error: 'A price cannot be negative.' });
});

it('should reject a price whose millisatoshi value cannot be represented exactly', () => {
  // 10^16 sats is 10^19 msats, far past Number.MAX_SAFE_INTEGER. JSON.stringify
  // would emit a value the daemon reads back as a different number.
  const parsed = parsePriceField('10000000000000000');

  expect(parsed).toEqual({ ok: false, error: 'That price is too large.' });
});

// Literals on both sides of the boundary. Deriving the expectation from
// Number.MAX_SAFE_INTEGER would reuse the arithmetic under test, so a wrong
// multiplier would move both sides together and the test would still pass.
it('should accept the largest price that survives the millisatoshi conversion', () => {
  expect(parsePriceField('9007199254740')).toEqual({ ok: true, priceMsat: 9_007_199_254_740_000 });
});

it('should reject the first price past the millisatoshi bound', () => {
  expect(parsePriceField('9007199254741')).toEqual({
    ok: false,
    error: 'That price is too large.'
  });
});

it('should describe no price as not selling', () => {
  expect(describeOffer(null)).toBe('Not selling seats');
});

it('should describe a zero price as free', () => {
  expect(describeOffer(0)).toBe('Free');
});

it('should describe a price per seat in grouped sats', () => {
  expect(describeOffer(50_000_000)).toBe('50,000 sats per seat');
});
