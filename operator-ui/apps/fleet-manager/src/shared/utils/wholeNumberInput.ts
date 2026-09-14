import { normalizeFieldText } from '@/shared/utils/fieldText';

/** Why a field was refused. Each caller words its own message, because the
 *  fields differ in what the number means and in where the error shows. */
export type WholeNumberRejection =
  | 'blank'
  | 'not-a-number'
  | 'fractional'
  | 'negative'
  | 'too-large';

export type ParsedWholeNumber =
  | { ok: true; value: number }
  | { ok: false; reason: WholeNumberRejection };

const DIGITS = /^\d+$/;
/** Whole groups of three split by one kind of separator. A partial group
 *  ("2,58") is a typo, not grouping. Normalizing has already turned no-break
 *  and thin spaces into plain ones. */
const GROUPED_DIGITS = /^\d{1,3}([, ])\d{3}(?:\1\d{3})*$/;
const LEADING_MINUS = /^[-\u2212]/;

const readDigits = (text: string): string | null => {
  if (DIGITS.test(text)) return text;
  if (GROUPED_DIGITS.test(text)) return text.replace(/[, ]/g, '');
  return null;
};

const isFraction = (text: string): boolean => {
  const [whole, fraction, ...rest] = text.split('.');
  if (fraction === undefined || rest.length > 0) return false;
  const hasDigit = `${whole}${fraction}` !== '';
  return hasDigit && (whole === '' || readDigits(whole) !== null) && /^\d*$/.test(fraction);
};

/**
 * Reads a whole number the way an operator writes one, including the grouped
 * form the dashboard prints ("2,587"). Stricter than `Number()`, which reads
 * `''` as 0, `0x10` as 16 and `1e3` as 1000.
 */
export const parseWholeNumberInput = (
  input: string,
  { max = Number.MAX_SAFE_INTEGER }: { max?: number } = {}
): ParsedWholeNumber => {
  const text = normalizeFieldText(input);
  if (text === '') return { ok: false, reason: 'blank' };

  const unsigned = text.replace(LEADING_MINUS, '');
  if (isFraction(unsigned)) return { ok: false, reason: 'fractional' };
  const digits = readDigits(unsigned);
  if (digits === null) return { ok: false, reason: 'not-a-number' };
  if (unsigned !== text) return { ok: false, reason: 'negative' };

  const value = Number(digits);
  if (!Number.isSafeInteger(value) || value > max) return { ok: false, reason: 'too-large' };
  return { ok: true, value };
};
