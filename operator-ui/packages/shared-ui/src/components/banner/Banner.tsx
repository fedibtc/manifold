import type { ReactNode } from 'react';
import styles from './Banner.module.css';

type BannerVariant = 'info' | 'warn' | 'error' | 'success';

interface BannerProps {
  variant: BannerVariant;
  title?: string;
  children: ReactNode;
  action?: ReactNode;
}

const glyphs: Record<BannerVariant, string> = {
  info: 'i',
  warn: '!',
  error: '!',
  success: '✓'
};

export const Banner = ({ variant, title, children, action }: BannerProps) => (
  <div className={styles.root} data-variant={variant}>
    <span className={styles.icon} aria-hidden="true">
      {glyphs[variant]}
    </span>

    <div className={styles.body}>
      {title && <strong className={styles.title}>{title}</strong>}
      <span>{children}</span>
    </div>

    {action && <div className={styles.action}>{action}</div>}
  </div>
);
