import type { ReactNode } from 'react';
import styles from './SectionCard.module.css';

export type SectionCardFrame = 'card' | 'table';

interface SectionCardProps {
  title: string;
  children: ReactNode;
  frame?: SectionCardFrame;
}

export const SectionCard = ({ title, children, frame = 'card' }: SectionCardProps) => (
  <section className={styles.root}>
    <h2 className={styles.title}>{title}</h2>

    <div className={styles.body} data-frame={frame}>
      {children}
    </div>
  </section>
);
