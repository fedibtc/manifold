import { useAuthPrompt } from '@/features/auth/hooks/use-auth-prompt/useAuthPrompt';
import styles from './AuthPromptPage.module.css';

// G2 — password sign-in (US-FMAN-001). The session lives in an HttpOnly cookie the
// browser manages, so there is nothing here to store or paste — only a password.
export const AuthPromptPage = () => {
  const { password, error, isSubmitting, onPasswordChange, onSubmit } = useAuthPrompt();

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        <div className={styles.glyph} aria-hidden="true">
          ●
        </div>

        <h1 className={styles.title}>Sign in</h1>

        <p className={styles.introText}>Enter the operator password to continue.</p>

        <form className={styles.form} onSubmit={onSubmit}>
          <label className={styles.fieldLabel} htmlFor="operator-password">
            Password
          </label>

          <input
            id="operator-password"
            type="password"
            className={styles.input}
            value={password}
            onChange={onPasswordChange}
          />

          {error ? <span className={styles.error}>{error}</span> : null}

          <button className={styles.submit} type="submit" disabled={isSubmitting}>
            Sign in
          </button>
        </form>
      </div>
    </div>
  );
};
