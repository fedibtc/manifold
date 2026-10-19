import type { ReactNode } from 'react';
import styles from './PeerBadgePhoneMockup.module.css';

interface PeerBadgePhoneMockupProps {
  src: string;
  alt: string;
  children?: ReactNode;
}

export const PeerBadgePhoneMockup = ({ src, alt, children }: PeerBadgePhoneMockupProps) => (
  <figure className={styles.root}>
    <div className={styles.device}>
      <div className={styles.status} aria-hidden="true">
        <span className={styles.time}>9:41</span>

        <span className={styles.island} />

        <svg className={styles.statusIcons} viewBox="0 0 34 10" aria-hidden="true">
          <path d="M1 9h1.6V7H1zM4 9h1.6V5.5H4zM7 9h1.6V4H7zM10 9h1.6V2.5H10z" />

          <path d="M15.5 4.2a5.4 5.4 0 0 1 7 0l-.9 1a4 4 0 0 0-5.2 0zM17.3 6.2a2.8 2.8 0 0 1 3.4 0L19 8z" />

          <rect x="24.5" y="2" width="8" height="6" rx="1.4" />
        </svg>
      </div>

      <div className={styles.screen}>
        <img className={styles.shot} src={src} alt={alt} loading="lazy" />
      </div>
    </div>

    <div className={styles.fade} aria-hidden="true" />

    {children}
  </figure>
);
