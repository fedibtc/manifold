import type { ReactNode } from 'react';
import styles from './DialogResult.module.css';

interface DialogResultProps {
  tone: 'success' | 'warning';
  heading: string;
  /** What the outcome means, as plain paragraphs, then any IDs or links. */
  children?: ReactNode;
}

/** The receipt: what happened, then what it means. */
export const DialogResult = ({ tone, heading, children }: DialogResultProps) => (
  <div className={styles.result} role="status">
    <span className={styles.glyph} data-tone={tone} aria-hidden="true">
      {tone === 'success' ? '✓' : '!'}
    </span>

    <p className={styles.heading}>{heading}</p>

    {children}
  </div>
);
