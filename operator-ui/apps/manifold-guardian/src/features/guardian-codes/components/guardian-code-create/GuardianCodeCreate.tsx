import { Banner, Button, CheckboxField, SectionCard, TextInput } from '@operator-ui/common-ui';
import type { SeatGrantEntry } from '@operator-ui/types';
import { type FormEvent, useState } from 'react';
import { useCreateSeatGrant } from '@/features/guardian-codes/api/hooks/use-create-seat-grant/useCreateSeatGrant';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './GuardianCodeCreate.module.css';

export const LABEL_MAX_CHARS = 32;

interface GuardianCodeCreateProps {
  onCreated: (grant: SeatGrantEntry) => void;
  onCancel: () => void;
}

// A code carries no price: setup through a guardian code is always free
// (REQ-bring-your-own-guardians), so there is nothing to choose here but a
// label and the delegation the operator accepts by sharing it.
export const GuardianCodeCreate = ({ onCreated, onCancel }: GuardianCodeCreateProps) => {
  const create = useCreateSeatGrant();
  const [label, setLabel] = useState('');
  const [trusted, setTrusted] = useState(false);
  const length = [...label.trim()].length;
  const tooLong = length > LABEL_MAX_CHARS;

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (tooLong || !trusted) return;
    const trimmed = label.trim();
    create.mutate(trimmed === '' ? null : trimmed, { onSuccess: onCreated });
  };

  return (
    <SectionCard title="Create a guardian code">
      <form className={styles.root} onSubmit={handleSubmit}>
        <p className={styles.intro}>
          A guardian code lets one Federation Ambassador add this guardian to one federation. Setup
          with a code is free.
        </p>

        <div className={styles.label}>
          <TextInput
            label="Label (optional)"
            value={label}
            onChange={setLabel}
            hint={`Shown to the Federation Ambassador as this guardian's name, and in your list below. ${length}/${LABEL_MAX_CHARS} characters.`}
            error={tooLong ? `Use ${LABEL_MAX_CHARS} characters or fewer.` : undefined}
            placeholder="For example, Lisbon meetup"
          />
        </div>

        <Banner variant="info" title="You are trusting the Federation Ambassador">
          Whoever uses this code chooses the other guardians in the federation, how many there are,
          and any later replacement of this seat. Share it only with someone you trust.
        </Banner>

        <CheckboxField
          label="I trust the Federation Ambassador I share this code with to choose the other guardians."
          checked={trusted}
          onChange={setTrusted}
        />

        {create.error ? <Banner variant="error">{describeActionError(create.error)}</Banner> : null}

        <div className={styles.actions}>
          <Button type="submit" disabled={!trusted || tooLong} loading={create.isPending}>
            Create guardian code
          </Button>

          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </form>
    </SectionCard>
  );
};
