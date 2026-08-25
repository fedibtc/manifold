import { type ChangeEvent, useId, useState } from 'react';
import styles from './ControlField.module.css';

export interface ControlFieldProps {
  /** Identifies the control to the commit handler, so the caller needs no
   *  per-row closure. */
  name: string;
  label: string;
  kind: 'number' | 'select';
  options?: readonly string[];
  value: string;
  onCommit: (name: string, value: string) => void;
}

export const ControlField = ({
  name,
  label,
  kind,
  options,
  value,
  onCommit
}: ControlFieldProps) => {
  // A number is typed through intermediate values that would each hit the mock,
  // so it commits on Apply. A select is a finished choice, so it commits at once.
  const [draft, setDraft] = useState(value);
  const inputId = useId();

  const handleSelect = (event: ChangeEvent<HTMLSelectElement>) =>
    onCommit(name, event.target.value);
  const handleType = (event: ChangeEvent<HTMLInputElement>) => setDraft(event.target.value);
  const handleApply = () => onCommit(name, draft);

  return (
    <div className={styles.field}>
      <label className={styles.label} htmlFor={inputId}>
        {label}
      </label>

      {kind === 'select' ? (
        <select className={styles.select} id={inputId} value={value} onChange={handleSelect}>
          {options?.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      ) : (
        <div className={styles.entry}>
          <input
            className={styles.input}
            id={inputId}
            type="number"
            min={0}
            value={draft}
            onChange={handleType}
          />

          {/* The Global tab carries more than one Apply, so each names its field. */}
          <button
            type="button"
            className={styles.apply}
            aria-label={`Apply ${label}`}
            onClick={handleApply}
          >
            Apply
          </button>
        </div>
      )}
    </div>
  );
};
