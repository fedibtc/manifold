import { expect, it } from 'vitest';
import { isPayoutDestinationFormat, normalizePayoutDestination } from '../payoutDestination';

it.each([
  ['surrounding spaces and a newline', ' operator@example.com\n', 'operator@example.com'],
  ['capitals from a phone keyboard', 'Operator@Example.com', 'operator@example.com'],
  ['a lightning: scheme', 'lightning:operator@example.com', 'operator@example.com'],
  ['an upper-case LNURL with its scheme', 'LIGHTNING:LNURL1DP68GURN8GHJ7', 'lnurl1dp68gurn8ghj7'],
  ['a zero-width space', 'operator\u200B@example.com', 'operator@example.com'],
  ['a full-width at sign', 'operator\uFF20example.com', 'operator@example.com']
])('should clean %s', (_case, input, expected) => {
  expect(normalizePayoutDestination(input)).toBe(expected);
});

it('should leave nothing to save from invisible characters alone', () => {
  expect(normalizePayoutDestination('\u200B\u00A0')).toBe('');
});

it.each([
  ['a Lightning address', 'operator@example.com'],
  ['a username with the characters LUD-16 allows', 'first.last_tips+1@pay.example.co.uk'],
  ['an onion domain', 'operator@exampleonionaddress.onion'],
  // The daemon's address parser accepts a single-label domain.
  ['a single-label domain', 'operator@localhost'],
  [
    'an LNURL',
    'lnurl1dp68gurn8ghj7um9wfmxjcm99e3k7mf0v9cxj0m385ekvcenxc6r2c35xvukxefcv5mkvv34x5ekzd3ev56nyd3hxqurzepexejxxepnxscrvwfnv9nxzcn9xq6xyefhvgcxxcmyxymnserxfq5fns'
  ]
])('should accept %s', (_case, destination) => {
  expect(isPayoutDestinationFormat(destination)).toBe(true);
});

it.each([
  ['a word', 'hello'],
  ['an address without a domain', 'operator@'],
  ['an address without a username', '@example.com'],
  ['two at signs', 'operator@@example.com'],
  ['a space inside', 'oper ator@example.com'],
  ['a domain with a port', 'operator@example.com:8080'],
  ['a bolt11 invoice', 'lnbc2500u1pvjluezpp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyq'],
  ['an on-chain address', 'bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq'],
  ['an LNURL prefix alone', 'lnurl'],
  ['an LNURL with a character bech32 does not use', 'lnurl1dp68gurn8ghj7b']
])('should refuse %s', (_case, destination) => {
  expect(isPayoutDestinationFormat(destination)).toBe(false);
});
