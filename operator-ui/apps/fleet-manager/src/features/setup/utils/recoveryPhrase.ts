import { normalizeFieldText } from '@/shared/utils/fieldText';

/**
 * The phrase as BIP-39 spells it: lowercase words, one space apart. The daemon
 * matches words exactly, so a capitalised first word from a phone keyboard or
 * one word per line from a password manager would otherwise be refused.
 */
export const normalizeRecoveryPhrase = (input: string): string =>
  normalizeFieldText(input).toLowerCase().split(/\s+/).join(' ');
