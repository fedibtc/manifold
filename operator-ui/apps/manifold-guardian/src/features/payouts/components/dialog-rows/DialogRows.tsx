import type { ReactNode } from 'react';
import styles from './DialogRows.module.css';

export interface DialogRow {
  label: string;
  value: ReactNode;
  /** Small grey text under the value: where a figure comes from. */
  caption?: string;
}

/** A withdrawal dialog's label/value list: what leaves, from where, to where. */
export const DialogRows = ({ rows }: { rows: DialogRow[] }) => (
  <dl className={styles.rows}>
    {rows.map((row) => (
      <div key={row.label} className={styles.row}>
        <dt className={styles.label}>{row.label}</dt>

        <dd className={styles.value}>
          {row.value}
          {row.caption && <span className={styles.caption}>{row.caption}</span>}
        </dd>
      </div>
    ))}
  </dl>
);
