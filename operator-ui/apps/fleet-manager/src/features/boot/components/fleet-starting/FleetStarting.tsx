import styles from './FleetStarting.module.css';

export const FleetStarting = () => (
  <div className={styles.page}>
    <div className={styles.card}>
      <span className={styles.spinner} role="status" aria-label="Starting" />

      <h1 className={styles.title}>Manifold Fedimint Guardian is starting</h1>

      <p className={styles.introText}>
        Your guardians and seats are safe. The dashboard opens when the fleet is ready.
      </p>
    </div>
  </div>
);
