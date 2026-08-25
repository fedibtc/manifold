import styles from './AccessDenied.module.css';

interface AccessDeniedProps {
  onRetry: () => void;
}

// Access-denied — a permission_denied ServiceError on an authenticated request
// (403). Distinct from AuthPromptPage (G2, 401): the operator token itself is
// valid, so this is never a re-auth prompt. Retry lets the operator re-check
// after a permission grant changes, without guessing at re-authentication.
export const AccessDenied = ({ onRetry }: AccessDeniedProps) => (
  <div className={styles.page}>
    <div className={styles.card}>
      <div className={styles.glyph} aria-hidden="true">
        ✕
      </div>

      <h1 className={styles.title}>This token can't access that</h1>

      <p className={styles.bodyIntro}>
        Your operator token is valid, but the daemon rejected this request as not permitted. Your
        funds and configuration are untouched — this is a permissions problem, not a connection
        problem.
      </p>

      <div className={styles.detail}>permission_denied</div>

      <button className={styles.retry} type="button" onClick={onRetry}>
        Retry
      </button>

      <p className={styles.help}>
        Still blocked? Check with whoever manages this installation's operator token — it may need a
        different role.
      </p>
    </div>
  </div>
);
