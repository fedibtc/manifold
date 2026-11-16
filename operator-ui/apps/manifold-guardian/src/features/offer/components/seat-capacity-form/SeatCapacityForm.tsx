import { Button, CheckboxField, SectionCard, TextInput } from '@operator-ui/common-ui';
import type { CapacityResponse } from '@operator-ui/types';
import { type FormEvent, useState } from 'react';
import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { useSetCapacity } from '@/shared/api/hooks/use-set-capacity/useSetCapacity';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeActionError } from '@/shared/utils/describeActionError';
import { MAX_SEAT_LIMIT } from '@/shared/utils/seatLimit';
import { parseWholeNumberInput } from '@/shared/utils/wholeNumberInput';
import styles from './SeatCapacityForm.module.css';

/**
 * `activeSeats` is the floor `Db::set_max_seats` enforces, counted the same way
 * it counts: seats that are not decommissioned. Pass `null` when the seat list
 * has not answered — the check is then skipped and the daemon's refusal remains
 * the guard, which it is in any case. Two operators, or a seat sold between the
 * read and the write, can still cross this floor after it passes here.
 */
export const parseSeatCapacity = (value: string, activeSeats: number | null = null) => {
  const parsed = parseWholeNumberInput(value, { max: MAX_SEAT_LIMIT });
  if (!parsed.ok) {
    return { ok: false as const, error: `Enter a whole number from 0 to ${MAX_SEAT_LIMIT}.` };
  }
  const maxSeats = parsed.value;
  if (activeSeats !== null && maxSeats < activeSeats) {
    return {
      ok: false as const,
      error: `You have ${activeSeats} active ${activeSeats === 1 ? 'seat' : 'seats'}. Decommission a seat before lowering the limit below that.`
    };
  }
  return { ok: true as const, maxSeats };
};

const seats = (count: number) => `${count} ${count === 1 ? 'seat' : 'seats'}`;

/** What the runtime and data limits allow together. While a CPU or data
 *  limit applies, it also names each applying limit's share side by side, so
 *  an operator sees why the host sells fewer seats than memory allows. */
export const describeAutomaticLimit = (capacity: CapacityResponse) => {
  const { memory_max_seats: memory, cpu_max_seats: cpu, data_max_seats: data } = capacity;
  const limit = Math.min(
    capacity.policy_max_seats,
    memory,
    cpu ?? Number.POSITIVE_INFINITY,
    data ?? Number.POSITIVE_INFINITY
  );
  const head = `Up to ${seats(limit)} for this host now.`;
  if (cpu === null && data === null) return head;
  const shares = [
    `Memory allows ${seats(memory)}`,
    ...(cpu === null ? [] : [`CPU ${seats(cpu)}`]),
    ...(data === null ? [] : [`monthly data ${seats(data)}`])
  ];
  return `${head} ${shares.join(', ')}.`;
};

/** Why a saved limit sells fewer seats than it says, when monthly data is
 *  below it and no runtime limit is lower; otherwise null. */
export const describeDataLimit = ({
  max_seats,
  memory_max_seats,
  policy_max_seats,
  cpu_max_seats,
  data_max_seats
}: CapacityResponse): string | null => {
  if (max_seats === null || data_max_seats === null) return null;
  if (data_max_seats >= max_seats) return null;
  const runtime = Math.min(
    memory_max_seats,
    policy_max_seats,
    cpu_max_seats ?? Number.POSITIVE_INFINITY
  );
  if (data_max_seats > runtime) return null;
  return `Your limit is ${seats(max_seats)}. Monthly data allows ${seats(data_max_seats)}.`;
};

export const SeatCapacityForm = () => {
  const capacity = useCapacity();
  const seats = useSeats();
  const setCapacity = useSetCapacity();
  const [automatic, setAutomatic] = useState(false);
  const [maxSeats, setMaxSeats] = useState('');
  const [hasSeeded, setHasSeeded] = useState(false);
  const [hasEdited, setHasEdited] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);

  if (!hasSeeded && capacity.data) {
    setAutomatic(capacity.data.max_seats === null);
    setMaxSeats(capacity.data.max_seats === null ? '' : String(capacity.data.max_seats));
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

  const handleAutomaticChange = (checked: boolean) => {
    setAutomatic(checked);
    setHasEdited(true);
    setValidationError(null);
  };

  // @owner: Keep this write separate from price; the daemon commits them independently.
  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!hasCapacity || !hasEdited) return;

    if (automatic) {
      setCapacity.mutate(null, { onSuccess: () => setHasEdited(false) });
      return;
    }
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

  const automaticHint = capacity.data ? describeAutomaticLimit(capacity.data) : undefined;

  // A saved limit above what monthly data allows sells only the data's seats.
  const dataLimitNote = capacity.data ? describeDataLimit(capacity.data) : null;

  return (
    <SectionCard title="Seat capacity">
      <QuerySurface disposition={disposition} onRetry={retry}>
        <form className={styles.form} onSubmit={handleSubmit}>
          <CheckboxField
            label="Automatic"
            hint={automaticHint}
            checked={automatic}
            onChange={handleAutomaticChange}
            disabled={setCapacity.isPending}
          />

          {automatic ? null : (
            <TextInput
              label="Maximum active seats"
              hint={hint}
              value={maxSeats}
              onChange={handleChange}
              error={error}
              disabled={setCapacity.isPending}
            />
          )}

          {automatic || dataLimitNote === null ? null : (
            <p className={styles.note}>{dataLimitNote}</p>
          )}

          {automatic && error ? <span className={styles.error}>{error}</span> : null}

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
