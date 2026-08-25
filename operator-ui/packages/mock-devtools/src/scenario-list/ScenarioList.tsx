import { ScenarioToggle } from '../scenario-toggle/ScenarioToggle';
import type { ScenarioCatalogEntry } from '../types';
import styles from './ScenarioList.module.css';

export interface ScenarioListProps {
  entries: readonly ScenarioCatalogEntry[];
  activeName: string;
  onSelect: (name: string) => void;
}

export const ScenarioList = ({ entries, activeName, onSelect }: ScenarioListProps) => (
  <ul className={styles.list}>
    {entries.map((entry) => (
      <ScenarioToggle
        key={entry.name}
        entry={entry}
        isActive={entry.name === activeName}
        onSelect={onSelect}
      />
    ))}
  </ul>
);
