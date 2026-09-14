import { type FormEvent, useState } from 'react';
import { useSetPayoutDestination } from '@/features/payouts/api/hooks/use-set-payout-destination/useSetPayoutDestination';
import {
  isPayoutDestinationFormat,
  normalizePayoutDestination
} from '@/features/payouts/utils/payoutDestination';
import { describeActionError } from '@/shared/utils/describeActionError';

const NOT_A_DESTINATION = 'Enter a Lightning address (name@example.com) or an LNURL (lnurl1…).';

export interface PayoutDestinationForm {
  value: string;
  onChange: (next: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClear: () => void;
  error: string | null;
  isPending: boolean;
  /** False while the field is blank: the daemon refuses an empty destination
   *  (crates/fman/core/src/fleet.rs:1113), and clearing is its own control. */
  canSave: boolean;
}

export const usePayoutDestinationForm = (destination: string | null): PayoutDestinationForm => {
  const setDestination = useSetPayoutDestination();
  const [value, setValue] = useState('');
  // Guarded setState during render, not in an effect — the sanctioned shape for
  // adjusting state on loaded data. Seeded once: re-seeding on every refetch
  // would overwrite an address the operator is part-way through typing.
  const [hasSeeded, setHasSeeded] = useState(false);
  if (!hasSeeded && destination !== null) {
    setValue(destination);
    setHasSeeded(true);
  }

  const [formatError, setFormatError] = useState<string | null>(null);

  const destinationToSave = normalizePayoutDestination(value);

  const onChange = (next: string) => {
    setValue(next);
    setFormatError(null);
  };

  const onSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (destinationToSave === '') return;
    if (!isPayoutDestinationFormat(destinationToSave)) {
      setFormatError(NOT_A_DESTINATION);
      return;
    }
    // The field shows what was stored, not what was typed, so it agrees with
    // the "Revenue leaves to" line above it.
    setDestination.mutate(destinationToSave, {
      onSuccess: (stored) => setValue(stored.destination ?? '')
    });
  };

  const onClear = () => {
    setValue('');
    setFormatError(null);
    setDestination.mutate(null);
  };

  return {
    value,
    onChange,
    onSubmit,
    onClear,
    error:
      formatError ?? (setDestination.isError ? describeActionError(setDestination.error) : null),
    isPending: setDestination.isPending,
    canSave: destinationToSave !== ''
  };
};
