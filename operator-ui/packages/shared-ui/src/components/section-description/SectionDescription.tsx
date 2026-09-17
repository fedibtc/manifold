import type { ReactNode } from 'react';
import styles from './SectionDescription.module.css';

interface SectionDescriptionProps {
  children: ReactNode;
}

export const SectionDescription = ({ children }: SectionDescriptionProps) => (
  <p className={styles.root}>{children}</p>
);
