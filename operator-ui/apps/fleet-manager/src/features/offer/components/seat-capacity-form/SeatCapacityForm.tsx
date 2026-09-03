import { Button, SectionCard, TextInput } from '@operator-ui/common-ui';
import { type FormEvent, useState } from 'react';
import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { useSetCapacity } from '@/shared/api/hooks/use-set-capacity/useSetCapacity';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './SeatCapacityForm.module.css';

const MAX_SEATS = 4_294_967_295;

export const parseSeatCapacity = (value: string) => {
  const maxSeats = Number(value.trim());
  if (!value.trim() || !Number.isInteger(maxSeats) || maxSeats < 0 || maxSeats > MAX_SEATS) {
    return { ok: false as const, error: `Enter a whole number from 0 to ${MAX_SEATS}.` };
  }
  return { ok: true as const, maxSeats };
};

export const SeatCapacityForm = () => {
  const capacity = useCapacity();
  const setCapacity = useSetCapacity();
  const [maxSeats, setMaxSeats] = useState('');
  const [hasSeeded, setHasSeeded] = useState(false);
  const [hasEdited, setHasEdited] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);

  if (!hasSeeded && capacity.data) {
    setMaxSeats(String(capacity.data.max_seats));
    setHasSeeded(true);
  }

  const { disposition, retry } = useQueryDisposition([capacity]);
  const hasCapacity = disposition.kind === 'content' || disposition.kind === 'stale';

  const handleChange = (value: string) => {
    setMaxSeats(value);
    setHasEdited(true);
    setValidationError(null);
  };

  // @owner: Keep this write separate from price; the daemon commits them independently.
  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!hasCapacity || !hasEdited) return;

    const parsed = parseSeatCapacity(maxSeats);
    if (!parsed.ok) {
      setValidationError(parsed.error);
      return;
    }
    setValidationError(null);
    setCapacity.mutate(parsed.maxSeats, { onSuccess: () => setHasEdited(false) });
  };

  const error =
    validationError ?? (setCapacity.isError ? describeActionError(setCapacity.error) : undefined);

  return (
    <SectionCard title="Seat capacity">
      <QuerySurface disposition={disposition} onRetry={retry}>
        <form className={styles.form} onSubmit={handleSubmit}>
          <TextInput
            label="Maximum active seats"
            type="number"
            min={0}
            value={maxSeats}
            onChange={handleChange}
            error={error}
            disabled={setCapacity.isPending}
          />

          <div className={styles.actions}>
            <Button
              type="submit"
              disabled={!hasCapacity || !hasEdited}
              loading={setCapacity.isPending}
            >
              Save seat limit
            </Button>
          </div>
        </form>
      </QuerySurface>
    </SectionCard>
  );
};
