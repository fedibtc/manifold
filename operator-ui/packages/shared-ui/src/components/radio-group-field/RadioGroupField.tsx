import { useEffect, useId, useRef } from 'react';
import styles from './RadioGroupField.module.css';

interface RadioOption<T extends string> {
  value: T;
  label: string;
}

interface RadioGroupFieldProps<T extends string> {
  /** The question the options answer. */
  label: string;
  options: readonly RadioOption<T>[];
  /** `null` while nothing is chosen: no option is preselected. */
  value: T | null;
  onChange: (value: T) => void;
  hint?: string;
  error?: string;
  disabled?: boolean;
}

/**
 * One choice among a few options, each a radio button beside its label. The
 * group is named by `label` and described by the hint and any error, so a
 * screen reader announces the question on entry. An error keeps the hint,
 * which says how to answer, and moves focus into the group: to the chosen
 * option, or the first.
 */
export const RadioGroupField = <T extends string>({
  label,
  options,
  value,
  onChange,
  hint,
  error,
  disabled = false
}: RadioGroupFieldProps<T>) => {
  const id = useId();
  const labelId = `${id}-label`;
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const describedBy =
    [hint ? hintId : null, error ? errorId : null].filter(Boolean).join(' ') || undefined;
  const optionsRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!error) return;
    const radios = optionsRef.current?.querySelectorAll<HTMLInputElement>('input[type="radio"]');
    const target = Array.from(radios ?? []).find((radio) => radio.checked) ?? radios?.[0];
    target?.focus();
  }, [error]);

  return (
    <div
      role="radiogroup"
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      aria-invalid={Boolean(error)}
      className={styles.field}
    >
      <span id={labelId} className={styles.label}>
        {label}
      </span>

      {hint && (
        <span id={hintId} className={styles.hint}>
          {hint}
        </span>
      )}

      {error && (
        <span id={errorId} role="alert" className={styles.error}>
          {error}
        </span>
      )}

      <div ref={optionsRef} className={styles.options}>
        {options.map((option) => {
          const optionId = `${id}-${option.value}`;
          return (
            <div key={option.value} className={styles.row}>
              <input
                id={optionId}
                type="radio"
                name={id}
                value={option.value}
                checked={value === option.value}
                disabled={disabled}
                onChange={() => onChange(option.value)}
                className={styles.radio}
              />

              <label htmlFor={optionId} className={styles.optionLabel}>
                {option.label}
              </label>
            </div>
          );
        })}
      </div>
    </div>
  );
};
