import styles from './PeerBadgeCallout.module.css';

interface PeerBadgeCalloutProps {
  src: string;
  alt: string;
}

export const PeerBadgeCallout = ({ src, alt }: PeerBadgeCalloutProps) => (
  <>
    <img className={styles.row} src={src} alt={alt} loading="lazy" />

    <svg className={styles.arrow} viewBox="0 0 56 24" aria-hidden="true">
      <path d="M2 14c16 8 34 6 50-6" />

      <path d="M44 5l8 3-4 7" />
    </svg>
  </>
);
