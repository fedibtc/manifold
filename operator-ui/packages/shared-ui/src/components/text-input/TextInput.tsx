import { useId } from 'react';
import styles from './TextInput.module.css';

interface TextInputProps {
  label: string;
  value: string;
  onChange: (v: string) => void;
  type?: 'text' | 'password' | 'number';
  /// Lower bound for `type="number"`. Advisory: the browser enforces it on
  /// stepper and spinner input, but a cleared or pasted box still reaches
  /// `onChange`, so the field's validator remains the guard.
  min?: number;
  placeholder?: string;
  hint?: string;
  error?: string;
  disabled?: boolean;
  id?: string;
  name?: string;
}

export const TextInput = ({
  label,
  value,
  onChange,
  type = 'text',
  min,
  placeholder,
  hint,
  error,
  disabled = false,
  id,
  name
}: TextInputProps) => {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const hintId = `${inputId}-hint`;
  const errorId = `${inputId}-error`;
  const describedBy = [!error && hint ? hintId : null, error ? errorId : null]
    .filter(Boolean)
    .join(' ');
  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    onChange(event.target.value);
  };
  return (
    <div className={styles.field}>
      <label htmlFor={inputId} className={styles.label}>
        {label}
      </label>
      {!error && hint && (
        <span id={hintId} className={styles.hint}>
          {hint}
        </span>
      )}

      <input
        id={inputId}
        name={name}
        type={type}
        min={min}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        onChange={handleChange}
        className={styles.input}
        data-invalid={Boolean(error)}
        aria-invalid={Boolean(error)}
        aria-describedby={describedBy || undefined}
      />
      {error && (
        <span id={errorId} role="alert" className={styles.error}>
          {error}
        </span>
      )}
    </div>
  );
};
