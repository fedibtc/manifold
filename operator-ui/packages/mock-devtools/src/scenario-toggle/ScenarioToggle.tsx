import type { ScenarioCatalogEntry } from '../types';
import styles from './ScenarioToggle.module.css';

export interface ScenarioToggleProps {
  entry: ScenarioCatalogEntry;
  isActive: boolean;
  onSelect: (name: string) => void;
}

export const ScenarioToggle = ({ entry, isActive, onSelect }: ScenarioToggleProps) => {
  const handleClick = () => onSelect(entry.name);

  return (
    <li className={styles.row}>
      <button
        type="button"
        role="switch"
        aria-checked={isActive}
        className={styles.control}
        onClick={handleClick}
      >
        <span className={styles.name}>{entry.name}</span>

        <span className={styles.track}>
          <span className={styles.thumb} />
        </span>
      </button>

      <p className={styles.desc}>{entry.desc}</p>
    </li>
  );
};
