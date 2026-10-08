import { expect, it } from 'vitest';
import { normalizeFieldText } from '../fieldText';

it.each([
  ['surrounding spaces', '  operator  ', 'operator'],
  ['a trailing newline from a paste', 'operator\n', 'operator'],
  ['surrounding no-break spaces', '\u00A0operator\u202F', 'operator'],
  ['a zero-width space', '\u200Boper\u200Bator', 'operator'],
  ['a byte order mark', '\uFEFFoperator', 'operator'],
  ['a direction mark', 'operator\u200E', 'operator'],
  ['a soft hyphen', 'oper\u00ADator', 'operator'],
  [
    'full-width letters and digits',
    '\uFF4F\uFF50\uFF45\uFF52\uFF41\uFF54\uFF4F\uFF52\uFF11\uFF12',
    'operator12'
  ]
])('should clean %s', (_case, input, expected) => {
  expect(normalizeFieldText(input)).toBe(expected);
});

it('should turn a no-break space between words into a plain space', () => {
  expect(normalizeFieldText('two\u00A0words')).toBe('two words');
});
