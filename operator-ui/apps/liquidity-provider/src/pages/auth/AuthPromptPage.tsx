import { useAuthPrompt } from '@/features/auth/hooks/use-auth-prompt/useAuthPrompt';
import styles from './AuthPromptPage.module.css';

// G2 — operator token prompt (14-error-auth.html). Blocking screen on 401 (AuthError).
export const AuthPromptPage = () => {
  const { value, onChange, onSubmit } = useAuthPrompt();

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        <div className={styles.glyph} aria-hidden="true">
          ●
        </div>

        <h1 className={styles.title}>Enter your operator token</h1>

        <p className={styles.bodyIntro}>
          This install isn't behind platform sign-in, so the dashboard needs the operator token from
          your setup notes or environment file.
        </p>

        <form className={styles.form} onSubmit={onSubmit}>
          <label className={styles.fieldLabel} htmlFor="admin-token">
            Admin token
          </label>

          <span className={styles.hint}>
            Kept in memory for this tab only — never stored in the browser. Sent as
            <span className={styles.mono}> Authorization: Bearer …</span>
          </span>

          <input
            id="admin-token"
            type="password"
            className={styles.input}
            placeholder="Paste your token"
            value={value}
            onChange={onChange}
          />

          <button className={styles.submit} type="submit">
            Continue
          </button>
        </form>
      </div>
    </div>
  );
};
