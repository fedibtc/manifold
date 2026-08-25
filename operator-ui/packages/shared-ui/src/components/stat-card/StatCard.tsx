import { Chip, type ChipTone } from '../chip/Chip';
import styles from './StatCard.module.css';

interface StatCardProps {
  label: string;
  value: string;
  hint?: string;
  chip?: string;
  chipTone?: ChipTone;
}

export const StatCard = ({ label, value, hint, chip, chipTone = 'neutral' }: StatCardProps) => (
  <div className={styles.root}>
    <div className={styles.label}>{label}</div>

    <div className={styles.value}>{value}</div>
    {chip ? (
      <Chip tone={chipTone}>{chip}</Chip>
    ) : hint ? (
      <div className={styles.hint}>{hint}</div>
    ) : null}
  </div>
);
