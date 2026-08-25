import { describe, expect, it } from 'vitest';
import { formatAmount, formatDate, formatSats, humanizeToken } from '../format';

describe('formatAmount', () => {
  it('should group thousands with en-US separators and no unit', () => {
    expect(formatAmount(1_234_567)).toBe('1,234,567');
  });

  it('should leave amounts below a thousand unchanged', () => {
    expect(formatAmount(750)).toBe('750');
  });

  it('should format zero as a single digit', () => {
    expect(formatAmount(0)).toBe('0');
  });

  it('should render an em dash rather than zero for an unreported amount', () => {
    expect(formatAmount(null)).toBe('—');
    expect(formatAmount(undefined)).toBe('—');
  });
});

describe('formatSats', () => {
  it('should group thousands and append the lowercase sats unit', () => {
    expect(formatSats(3_250_000)).toBe('3,250,000 sats');
  });

  it('should render zero as a plain amount', () => {
    expect(formatSats(0)).toBe('0 sats');
  });

  it('should render an em dash rather than zero for an unreported amount', () => {
    expect(formatSats(null)).toBe('—');
    expect(formatSats(undefined)).toBe('—');
  });
});

describe('humanizeToken', () => {
  it('should turn a snake_case wire token into a spaced label', () => {
    expect(humanizeToken('gateway_funding')).toBe('gateway funding');
  });
});

describe('formatDate', () => {
  it('should format a numeric Unix-seconds timestamp as an ISO date', () => {
    expect(formatDate(1721476800)).toBe('2024-07-20'); // date -u -r 1721476800 → Sat Jul 20 12:00:00 UTC 2024
  });
});
