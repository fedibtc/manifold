import type { ReactNode, Ref } from 'react';
import styles from './Button.module.css';

interface ButtonProps {
  variant?: 'primary' | 'secondary' | 'danger';
  size?: 'medium' | 'small';
  type?: 'button' | 'submit' | 'reset';
  disabled?: boolean;
  loading?: boolean;
  fullWidth?: boolean;
  /** Id of the element explaining this button — typically why it is disabled. */
  describedBy?: string;
  /** For callers that must move focus here — e.g. returning focus to the
   *  trigger after a confirmation panel closes. */
  ref?: Ref<HTMLButtonElement>;
  onClick?: () => void;
  children: ReactNode;
}

export const Button = ({
  variant = 'primary',
  size = 'medium',
  type = 'button',
  disabled = false,
  loading = false,
  fullWidth = false,
  describedBy,
  ref,
  onClick,
  children
}: ButtonProps) => {
  // A busy button is not an unavailable one. Repainting it in the inactive
  // variant and widening it for the spinner made a short request read as a
  // flicker, so loading keeps the button's own size and colours and only marks
  // itself busy; the spinner sits over the label instead of beside it.
  return (
    <button
      ref={ref}
      type={type}
      className={styles.root}
      data-variant={disabled ? 'inactive' : variant}
      data-size={size}
      data-full-width={fullWidth}
      data-loading={loading}
      disabled={disabled || loading}
      aria-busy={loading}
      aria-describedby={describedBy}
      onClick={onClick}
    >
      {loading && <span className={styles.spinner} aria-hidden="true" />}
      <span className={styles.label}>{children}</span>
    </button>
  );
};
