import { Button, SectionCard, TextInput } from '@operator-ui/common-ui';
import { type FormEvent, useState } from 'react';
import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { useSetCapacity } from '@/shared/api/hooks/use-set-capacity/useSetCapacity';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './SeatCapacityForm.module.css';

const MAX_SEATS = 4_294_967_295;

/**
 * `activeSeats` is the floor `Db::set_max_seats` enforces, counted the same way
 * it counts: seats that are not decommissioned. Pass `null` when the seat list
 * has not answered — the check is then skipped and the daemon's refusal remains
 * the guard, which it is in any case. Two operators, or a seat sold between the
 * read and the write, can still cross this floor after it passes here.
 */
export const parseSeatCapacity = (value: string, activeSeats: number | null = null) => {
  const maxSeats = Number(value.trim());
  if (!value.trim() || !Number.isInteger(maxSeats) || maxSeats < 0 || maxSeats > MAX_SEATS) {
    return { ok: false as const, error: `Enter a whole number from 0 to ${MAX_SEATS}.` };
  }
  if (activeSeats !== null && maxSeats < activeSeats) {
    return {
      ok: false as const,
      error: `You have ${activeSeats} active ${activeSeats === 1 ? 'seat' : 'seats'}. Decommission a seat before lowering the limit below that.`
    };
  }
  return { ok: true as const, maxSeats };
};

export const SeatCapacityForm = () => {
  const capacity = useCapacity();
  const seats = useSeats();
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

  // Deliberately outside the disposition above: the seat list only sharpens the
  // error the operator gets, so a failed seat read must not take the capacity
  // field away. Without it the daemon still refuses, one round trip later.
  const activeSeats = seats.data
    ? seats.data.seats.filter((seat) => !seat.decommissioned).length
    : null;

  const handleChange = (value: string) => {
    setMaxSeats(value);
    setHasEdited(true);
    setValidationError(null);
  };

  // @owner: Keep this write separate from price; the daemon commits them independently.
  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!hasCapacity || !hasEdited) return;

    const parsed = parseSeatCapacity(maxSeats, activeSeats);
    if (!parsed.ok) {
      setValidationError(parsed.error);
      return;
    }
    setValidationError(null);
    setCapacity.mutate(parsed.maxSeats, { onSuccess: () => setHasEdited(false) });
  };

  const error =
    validationError ?? (setCapacity.isError ? describeActionError(setCapacity.error) : undefined);

  // Says the floor before the operator hits it, so the refusal is the rare case
  // rather than the way they find out the rule exists.
  const hint =
    activeSeats === null
      ? undefined
      : `${activeSeats} ${activeSeats === 1 ? 'seat is' : 'seats are'} active. The limit cannot go below that.`;

  return (
    <SectionCard title="Seat capacity">
      <QuerySurface disposition={disposition} onRetry={retry}>
        <form className={styles.form} onSubmit={handleSubmit}>
          <TextInput
            label="Maximum active seats"
            type="number"
            min={activeSeats ?? 0}
            hint={hint}
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
