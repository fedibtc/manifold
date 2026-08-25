import { type FormEvent, useState } from 'react';
import { useSetPayoutDestination } from '@/features/payouts/api/hooks/use-set-payout-destination/useSetPayoutDestination';
import { describeActionError } from '@/shared/utils/describeActionError';

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

  const onSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const trimmed = value.trim();
    if (trimmed === '') return;
    setDestination.mutate(trimmed);
  };

  const onClear = () => {
    setValue('');
    setDestination.mutate(null);
  };

  return {
    value,
    onChange: setValue,
    onSubmit,
    onClear,
    error: setDestination.isError ? describeActionError(setDestination.error) : null,
    isPending: setDestination.isPending,
    canSave: value.trim() !== ''
  };
};
