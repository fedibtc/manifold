import { useId } from 'react';
import styles from './CheckboxField.module.css';

interface CheckboxFieldProps {
  label: string;
  checked: boolean;
  onChange: (b: boolean) => void;
  hint?: string;
  disabled?: boolean;
}

export const CheckboxField = ({
  label,
  checked,
  onChange,
  hint,
  disabled = false
}: CheckboxFieldProps) => {
  const inputId = useId();
  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    onChange(event.target.checked);
  };
  return (
    <div className={styles.wrapper}>
      <div className={styles.row}>
        <input
          id={inputId}
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={handleChange}
          className={styles.box}
        />

        <label htmlFor={inputId} className={styles.label}>
          {label}
        </label>
      </div>
      {hint && <span className={styles.hint}>{hint}</span>}
    </div>
  );
};
