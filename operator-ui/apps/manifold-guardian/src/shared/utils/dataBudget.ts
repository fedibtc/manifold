import type { MonthlyDataBudget } from '@operator-ui/types';
import { parseWholeNumberInput } from '@/shared/utils/wholeNumberInput';

/** The daemon stores a GB budget as a positive u32. */
export const MAX_DATA_BUDGET_GB = 4_294_967_295;

/** The operator's answer to {@link DATA_CAP_QUESTION}. */
export type DataCapChoice = 'unlimited' | 'limited';

export const DATA_CAP_QUESTION =
  'Does your internet plan limit how much data you can use each month?';

export const DATA_CAP_OPTIONS = [
  { value: 'unlimited', label: 'No monthly limit' },
  { value: 'limited', label: 'Yes, my plan has a monthly limit' }
] as const satisfies readonly { value: DataCapChoice; label: string }[];

export const DATA_BUDGET_LABEL = 'Monthly data for this guardian (GB)';

/** What the amount does, and that it is not a hard limit. Shown wherever the
 *  operator answers the question. */
export const DATA_BUDGET_EXPLAINER =
  'If your plan has a limit, this host stops selling new seats once its seats would use about the amount you enter. ' +
  'It is not a hard limit: seats you already run keep running, and a busy federation can use more than estimated.';

/** The units of the amount, said once, in the amount's help text. */
export const DATA_AMOUNT_HINT =
  'How much of your plan this guardian may use, in GB per 30 days (1 GB is 1,000,000,000 bytes).';

const NO_CHOICE = 'Choose whether your internet plan has a monthly data limit.';
const INVALID_BUDGET = 'Enter a whole number of GB, such as 500.';

export type ParsedDataBudget =
  | { ok: true; budget: Exclude<MonthlyDataBudget, 'not_set'> }
  | { ok: false; field: 'choice' | 'gb'; error: string };

/** The answer the daemon stores. Zero is refused rather than read as "sell
 *  nothing": the seat limit is the setting for that. */
export const parseDataBudget = (choice: DataCapChoice | null, gb: string): ParsedDataBudget => {
  if (choice === null) return { ok: false, field: 'choice', error: NO_CHOICE };
  if (choice === 'unlimited') return { ok: true, budget: 'unlimited' };
  const parsed = parseWholeNumberInput(gb, { max: MAX_DATA_BUDGET_GB });
  if (!parsed.ok || parsed.value === 0) return { ok: false, field: 'gb', error: INVALID_BUDGET };
  return { ok: true, budget: `${parsed.value}` };
};

/** The GB that limit sales, or null when no data limit applies. */
export const budgetGb = (budget: MonthlyDataBudget): number | null =>
  budget === 'not_set' || budget === 'unlimited' ? null : Number(budget);

/** The choice a stored answer shows; nothing is chosen before an answer. */
export const choiceOf = (budget: MonthlyDataBudget): DataCapChoice | null => {
  if (budget === 'not_set') return null;
  return budget === 'unlimited' ? 'unlimited' : 'limited';
};

export const formatGb = (gb: number): string => `${gb.toLocaleString('en-US')} GB`;

export const seatCount = (seats: number): string =>
  `${seats.toLocaleString('en-US')} ${seats === 1 ? 'seat' : 'seats'}`;

/** The per-seat figure as the operator reads it. */
export const describeSeatEstimate = (seatMonthlyDataGb: number): string =>
  `We estimate one seat uses about ${formatGb(seatMonthlyDataGb)} a month.`;

/** For a budget that covers no seat: the MG takes it and sells no new seat.
 *  Not "full", because no seat need be running. */
export const describeBelowOneSeat = (budget: number, seatMonthlyDataGb: number): string =>
  `${formatGb(budget)} is less than one seat (about ${formatGb(seatMonthlyDataGb)} a month). ` +
  'This guardian will not sell new seats.';
