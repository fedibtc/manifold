import { expect, it } from 'vitest';
import { normalizeRecoveryPhrase } from '../recoveryPhrase';

it.each([
  ['surrounding spaces', '  abandon about  ', 'abandon about'],
  ['a capitalised first word', 'Abandon about', 'abandon about'],
  ['upper-case words', 'ABANDON ABOUT', 'abandon about'],
  ['one word per line', 'abandon\nabout\n', 'abandon about'],
  ['tabs and repeated spaces', 'abandon\t  about', 'abandon about'],
  ['a no-break space between words', 'abandon\u00A0about', 'abandon about'],
  ['a zero-width space', 'aban\u200Bdon about', 'abandon about']
])('should clean %s', (_case, input, expected) => {
  expect(normalizeRecoveryPhrase(input)).toBe(expected);
});

it('should leave nothing to restore from whitespace alone', () => {
  expect(normalizeRecoveryPhrase(' \n\u200B ')).toBe('');
});
