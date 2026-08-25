import type { ReactNode } from 'react';
import styles from './PageShell.module.css';

interface PageShellProps {
  title: string;
  nav?: ReactNode;
  children: ReactNode;
}

export const PageShell = ({ title, nav, children }: PageShellProps) => (
  <div className={styles.root}>
    <header className={styles.header}>
      <h1 className={styles.title}>{title}</h1>
      {nav}
    </header>

    <main className={styles.main}>{children}</main>
  </div>
);
