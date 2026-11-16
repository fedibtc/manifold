import { Banner, Button, SectionCard } from '@operator-ui/common-ui';
import { type FormEvent, useState } from 'react';
import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { useSetDataBudget } from '@/shared/api/hooks/use-set-data-budget/useSetDataBudget';
import { MonthlyDataField } from '@/shared/components/monthly-data-field/MonthlyDataField';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import {
  budgetGb,
  choiceOf,
  DATA_AMOUNT_HINT,
  DATA_BUDGET_EXPLAINER,
  type DataCapChoice,
  describeBelowOneSeat,
  describeSeatEstimate,
  formatGb,
  parseDataBudget,
  seatCount
} from '@/shared/utils/dataBudget';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './DataBudgetForm.module.css';

interface FieldErrors {
  choice?: string;
  gb?: string;
}

export const DataBudgetForm = () => {
  const capacity = useCapacity();
  const setDataBudget = useSetDataBudget();
  const [choice, setChoice] = useState<DataCapChoice | null>(null);
  const [gb, setGb] = useState('');
  const [hasSeeded, setHasSeeded] = useState(false);
  const [hasEdited, setHasEdited] = useState(false);
  const [errors, setErrors] = useState<FieldErrors>({});

  if (!hasSeeded && capacity.data) {
    const stored = capacity.data.monthly_data_budget;
    setChoice(choiceOf(stored));
    setGb(budgetGb(stored)?.toString() ?? '');
    setHasSeeded(true);
  }

  const { disposition, retry } = useQueryDisposition([capacity]);
  const hasCapacity = disposition.kind === 'content' || disposition.kind === 'stale';

  const edit = () => {
    setHasEdited(true);
    setErrors({});
  };

  const handleChoiceChange = (value: DataCapChoice) => {
    setChoice(value);
    edit();
  };

  const handleGbChange = (value: string) => {
    setGb(value);
    edit();
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!hasCapacity || !hasEdited) return;
    const parsed = parseDataBudget(choice, gb);
    if (!parsed.ok) {
      setErrors({ [parsed.field]: parsed.error });
      return;
    }
    setErrors({});
    setDataBudget.mutate(parsed.budget, { onSuccess: () => setHasEdited(false) });
  };

  const actionError = setDataBudget.isError ? describeActionError(setDataBudget.error) : undefined;

  // What the saved amount does now, from the daemon's own division.
  const storedGb = capacity.data ? budgetGb(capacity.data.monthly_data_budget) : null;
  const dataMaxSeats = capacity.data?.data_max_seats ?? null;
  const saved = !hasEdited && storedGb !== null && dataMaxSeats !== null;
  const belowOneSeat =
    saved && capacity.data && dataMaxSeats === 0
      ? describeBelowOneSeat(storedGb, capacity.data.seat_monthly_data_gb)
      : null;
  const coverage =
    saved && dataMaxSeats > 0 ? `${formatGb(storedGb)} covers ${seatCount(dataMaxSeats)}.` : null;

  return (
    <SectionCard title="Monthly data">
      <QuerySurface disposition={disposition} onRetry={retry}>
        <form className={styles.form} onSubmit={handleSubmit}>
          <MonthlyDataField
            choice={choice}
            onChoiceChange={handleChoiceChange}
            gb={gb}
            onGbChange={handleGbChange}
            hint={DATA_BUDGET_EXPLAINER}
            gbHint={
              capacity.data
                ? `${DATA_AMOUNT_HINT} ${describeSeatEstimate(capacity.data.seat_monthly_data_gb)}`
                : DATA_AMOUNT_HINT
            }
            choiceError={errors.choice ?? (choice === 'limited' ? undefined : actionError)}
            gbError={errors.gb ?? (choice === 'limited' ? actionError : undefined)}
            coverage={coverage}
            disabled={setDataBudget.isPending}
          />

          {belowOneSeat ? <Banner variant="warn">{belowOneSeat}</Banner> : null}

          <div className={styles.actions}>
            <Button
              type="submit"
              disabled={!hasCapacity || !hasEdited}
              loading={setDataBudget.isPending}
            >
              Save monthly data
            </Button>
          </div>
        </form>
      </QuerySurface>
    </SectionCard>
  );
};
