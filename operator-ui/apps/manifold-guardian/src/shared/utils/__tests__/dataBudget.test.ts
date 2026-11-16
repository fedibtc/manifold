import { expect, it } from 'vitest';
import { budgetGb, choiceOf, parseDataBudget } from '../dataBudget';

const INVALID = { ok: false, field: 'gb', error: 'Enter a whole number of GB, such as 500.' };

it('should need an answer before anything else', () => {
  expect(parseDataBudget(null, '500')).toEqual({
    ok: false,
    field: 'choice',
    error: 'Choose whether your internet plan has a monthly data limit.'
  });
});

it('should ignore the amount when the plan has no limit', () => {
  expect(parseDataBudget('unlimited', 'not a number')).toEqual({
    ok: true,
    budget: 'unlimited'
  });
});

it.each([
  ['500', '500'],
  ['1,500', '1500'],
  ['4294967295', '4294967295']
])('should read %j as the token %j', (value, budget) => {
  expect(parseDataBudget('limited', value)).toEqual({ ok: true, budget });
});

// A limited plan needs an amount, and the daemon stores a positive u32.
it.each([
  '',
  '  ',
  '0',
  '-1',
  '2.5',
  '4294967296',
  '1e3',
  '500 GB'
])('should refuse %j for a limited plan', (value) => {
  expect(parseDataBudget('limited', value)).toEqual(INVALID);
});

it('should read only a GB answer as a limit', () => {
  expect(budgetGb('179')).toBe(179);
  expect(budgetGb('unlimited')).toBeNull();
  expect(budgetGb('not_set')).toBeNull();
  expect(choiceOf('179')).toBe('limited');
  expect(choiceOf('unlimited')).toBe('unlimited');
  expect(choiceOf('not_set')).toBeNull();
});
