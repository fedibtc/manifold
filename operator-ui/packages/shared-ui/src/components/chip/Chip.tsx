import type { ReactNode } from 'react';
import styles from './Chip.module.css';

export type ChipTone = 'ok' | 'warn' | 'bad' | 'info' | 'neutral';

interface ChipProps {
  tone: ChipTone;
  children: ReactNode;
}

export const Chip = ({ tone, children }: ChipProps) => (
  <span className={styles.root} data-tone={tone}>
    {children}
  </span>
);
