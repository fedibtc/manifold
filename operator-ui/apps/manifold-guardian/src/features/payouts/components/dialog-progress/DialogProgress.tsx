import styles from './DialogProgress.module.css';

interface ProgressStep {
  label: string;
  done: boolean;
}

/** What the money move is doing now, the running step last. */
export const DialogProgress = ({ steps }: { steps: ProgressStep[] }) => (
  <ol className={styles.progress} aria-live="polite">
    {steps.map((step) => (
      <li key={step.label} className={step.done ? styles.stepDone : styles.stepNow}>
        {step.done ? (
          <span className={styles.tick} aria-hidden="true">
            ✓
          </span>
        ) : (
          <span className={styles.spinner} aria-hidden="true" />
        )}
        {step.label}
      </li>
    ))}
  </ol>
);
