import { describe, expect, it } from 'vitest';
import { parseWholeNumberInput } from '../wholeNumberInput';

describe('parseWholeNumberInput', () => {
  it.each([
    ['0', 0],
    ['2587', 2587],
    [' 2587 ', 2587],
    ['2,587', 2587],
    ['1,000,000', 1_000_000],
    ['2 587', 2587],
    ['2\u00A0587', 2587],
    ['2\u202F587', 2587],
    ['\u200B2587', 2587],
    ['\uFF12\uFF15\uFF18\uFF17', 2587]
  ])('should read %j as %d', (input, expected) => {
    expect(parseWholeNumberInput(input)).toEqual({ ok: true, value: expected });
  });

  it.each(['', '   ', '\u200B'])('should report %j as blank', (input) => {
    expect(parseWholeNumberInput(input)).toEqual({ ok: false, reason: 'blank' });
  });

  // A separator only groups whole threes. Anything else is a typo the operator
  // should see, not a number to guess at. `Number()` would read the last four
  // as 16, 1000, 5 and Infinity.
  it.each([
    'lots',
    '2,5',
    '2,58',
    ',587',
    '2,,587',
    '1,000,00',
    '2,587 000',
    '0x10',
    '1e3',
    '+5',
    'Infinity'
  ])('should refuse %j as not a number', (input) => {
    expect(parseWholeNumberInput(input)).toEqual({ ok: false, reason: 'not-a-number' });
  });

  it.each(['2.5', '.5', '5.', '12.0', '2,587.5'])('should refuse %j as fractional', (input) => {
    expect(parseWholeNumberInput(input)).toEqual({ ok: false, reason: 'fractional' });
  });

  it.each(['-1', '-0', '\u22125', '-2,587'])('should refuse %j as negative', (input) => {
    expect(parseWholeNumberInput(input)).toEqual({ ok: false, reason: 'negative' });
  });

  it('should refuse a value past the given maximum', () => {
    expect(parseWholeNumberInput('4,294,967,296', { max: 4_294_967_295 })).toEqual({
      ok: false,
      reason: 'too-large'
    });
  });

  it('should accept the maximum itself', () => {
    expect(parseWholeNumberInput('4,294,967,295', { max: 4_294_967_295 })).toEqual({
      ok: true,
      value: 4_294_967_295
    });
  });

  it('should refuse a value a JavaScript number cannot hold exactly', () => {
    expect(parseWholeNumberInput('9007199254740993')).toEqual({ ok: false, reason: 'too-large' });
  });
});
