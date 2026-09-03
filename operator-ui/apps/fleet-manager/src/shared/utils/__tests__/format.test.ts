import { formatDate, formatSats } from '../format';

it('should convert msat to sats and label the unit', () => {
  expect(formatSats(250_000_000)).toBe('250,000 sats');
});

it('should floor a partial-sat msat amount', () => {
  expect(formatSats(1_500)).toBe('1 sats');
});

it('should render an em dash rather than zero for an unreported amount', () => {
  expect(formatSats(null)).toBe('—');
  expect(formatSats(undefined)).toBe('—');
});

it('should still render a genuine zero balance as zero', () => {
  expect(formatSats(0)).toBe('0 sats');
});

it('should preserve exact wire amounts beyond JavaScript safe integers', () => {
  expect(formatSats('18446744073709551615')).toBe('18,446,744,073,709,551 sats');
});

it('should format a millisecond timestamp as an ISO date', () => {
  expect(formatDate(1_753_000_000_000)).toBe('2025-07-20');
});
