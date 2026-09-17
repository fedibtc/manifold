import type { ReactNode } from 'react';
import styles from './SectionDescription.module.css';

interface SectionDescriptionProps {
  children: ReactNode;
}

/**
 * One line under a section title saying what the section is about. Separate from
 * `SectionCard` because a card frames whatever it is given, while this states
 * what that content means — two responsibilities a section may want either of.
 */
export const SectionDescription = ({ children }: SectionDescriptionProps) => (
  <p className={styles.root}>{children}</p>
);
