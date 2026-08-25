import { useId } from 'react';
import styles from './SelectField.module.css';

interface SelectOption {
  value: string;
  label: string;
}

interface SelectFieldProps {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: SelectOption[];
  hint?: string;
  error?: string;
  disabled?: boolean;
  id?: string;
}

export const SelectField = ({
  label,
  value,
  onChange,
  options,
  hint,
  error,
  disabled = false,
  id
}: SelectFieldProps) => {
  const generatedId = useId();
  const selectId = id ?? generatedId;
  const hintId = `${selectId}-hint`;
  const errorId = `${selectId}-error`;
  const describedBy = [!error && hint ? hintId : null, error ? errorId : null]
    .filter(Boolean)
    .join(' ');
  const handleChange = (event: React.ChangeEvent<HTMLSelectElement>) => {
    onChange(event.target.value);
  };
  return (
    <div className={styles.field}>
      <label htmlFor={selectId} className={styles.label}>
        {label}
      </label>
      {!error && hint && (
        <span id={hintId} className={styles.hint}>
          {hint}
        </span>
      )}

      <select
        id={selectId}
        value={value}
        disabled={disabled}
        onChange={handleChange}
        className={styles.select}
        data-invalid={Boolean(error)}
        aria-invalid={Boolean(error)}
        aria-describedby={describedBy || undefined}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {error && (
        <span id={errorId} role="alert" className={styles.error}>
          {error}
        </span>
      )}
    </div>
  );
};
