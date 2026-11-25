import type { KeyboardEvent } from 'react';
import styles from './MethodSwitch.module.css';

export type PayoutMethod = 'lightning' | 'ecash';

const OPTIONS: { value: PayoutMethod; label: string }[] = [
  { value: 'lightning', label: 'Lightning' },
  { value: 'ecash', label: 'Ecash' }
];

interface MethodSwitchProps {
  value: PayoutMethod;
  onChange: (method: PayoutMethod) => void;
}

/** How a withdrawal leaves: a segmented radio group, arrow keys move it. */
export const MethodSwitch = ({ value, onChange }: MethodSwitchProps) => {
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
    event.preventDefault();
    const next = value === 'lightning' ? 'ecash' : 'lightning';
    onChange(next);
    event.currentTarget.querySelector<HTMLButtonElement>(`[data-value="${next}"]`)?.focus();
  };

  return (
    <div
      className={styles.root}
      role="radiogroup"
      aria-label="How to withdraw"
      onKeyDown={handleKeyDown}
    >
      {OPTIONS.map((option) => (
        // A segmented control: two styled segments with arrow-key movement
        // between them. Native radios would need a hidden input per segment
        // to look like this, and the roles already give screen readers the
        // radiogroup semantics.
        // biome-ignore lint/a11y/useSemanticElements: see the note above
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={value === option.value}
          tabIndex={value === option.value ? 0 : -1}
          data-value={option.value}
          data-on={value === option.value}
          className={styles.segment}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
};
