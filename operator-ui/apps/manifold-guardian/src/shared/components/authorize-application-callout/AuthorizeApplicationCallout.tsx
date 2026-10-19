import authorizeApplicationRow from '@/shared/assets/peerbadge/authorize-application-row.webp';
import styles from './AuthorizeApplicationCallout.module.css';

export const AuthorizeApplicationCallout = () => (
  <div className={styles.root}>
    <img
      className={styles.row}
      src={authorizeApplicationRow}
      alt="Authorize application: let an app use only this badge"
      loading="lazy"
    />

    <svg className={styles.arrow} viewBox="0 0 56 24" aria-hidden="true">
      <path d="M2 14c16 8 34 6 50-6" />

      <path d="M44 5l8 3-4 7" />
    </svg>
  </div>
);
