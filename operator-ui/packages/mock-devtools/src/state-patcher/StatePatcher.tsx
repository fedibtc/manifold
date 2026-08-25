import { type ChangeEvent, useId, useState } from 'react';
import styles from './StatePatcher.module.css';

export interface StatePatcherProps {
  onPatch: (path: string, value: unknown) => void;
}

export const StatePatcher = ({ onPatch }: StatePatcherProps) => {
  const [path, setPath] = useState('');
  const [raw, setRaw] = useState('');
  const [error, setError] = useState<string | null>(null);
  const pathId = useId();
  const valueId = useId();

  const handlePathChange = (event: ChangeEvent<HTMLInputElement>) => setPath(event.target.value);
  const handleRawChange = (event: ChangeEvent<HTMLInputElement>) => setRaw(event.target.value);

  const handleApply = () => {
    let value: unknown;
    try {
      value = JSON.parse(raw);
    } catch {
      // A malformed value is the common typo here, and losing the panel to an
      // exception would be a worse outcome than saying so in place.
      setError('value must be JSON');
      return;
    }

    setError(null);
    onPatch(path, value);
  };

  return (
    <div className={styles.patcher}>
      <label className={styles.label} htmlFor={pathId}>
        Path
      </label>

      <input
        className={styles.input}
        id={pathId}
        value={path}
        placeholder="seats.0.report.health"
        onChange={handlePathChange}
      />

      <label className={styles.label} htmlFor={valueId}>
        Value
      </label>

      <div className={styles.entry}>
        <input
          className={styles.input}
          id={valueId}
          value={raw}
          placeholder='"unavailable"'
          onChange={handleRawChange}
        />

        <button
          type="button"
          className={styles.apply}
          aria-label="Apply patch"
          onClick={handleApply}
        >
          Apply
        </button>
      </div>

      {error ? <p className={styles.error}>{error}</p> : null}
    </div>
  );
};
