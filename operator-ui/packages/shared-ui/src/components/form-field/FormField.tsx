import { type ReactNode, useId } from 'react';
import styles from './FormField.module.css';

interface FormFieldProps {
  label: string;
  hint?: string;
  error?: string;
  children: (ids: { describedBy?: string }) => ReactNode;
}

/**
 * A labelled *group* of controls — a key/value editor, a checkbox set, a list of
 * relay inputs. Deliberately not a `<label htmlFor>`: every caller wraps more
 * than one control, and a `<label>` may only name one, so pointing it at an
 * arbitrary child would misname the rest. The group is named as a whole
 * instead, and the hint/error are described on it so they are announced on
 * entry rather than being loose text a screen reader meets after the controls.
 *
 * `describedBy` is still handed to children so a caller wrapping exactly one
 * control can associate it directly.
 */
export const FormField = ({ label, hint, error, children }: FormFieldProps) => {
  const id = useId();
  const labelId = `${id}-label`;
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  // An error replaces the hint on screen, so it must replace it in the
  // description too — otherwise a screen reader announces guidance that is no
  // longer visible.
  const describedBy =
    [!error && hint ? hintId : null, error ? errorId : null].filter(Boolean).join(' ') || undefined;

  return (
    // <fieldset> is the semantic equivalent, but its <legend> is excluded from
    // flex layout, so swapping it in silently drops the 4px label gap this
    // field stack is specified with. role="group" + aria-labelledby is
    // identical to assistive tech and keeps the wireframe spacing.
    // biome-ignore lint/a11y/useSemanticElements: see the note above
    <div
      role="group"
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      className={styles.field}
    >
      <span id={labelId} className={styles.label}>
        {label}
      </span>

      {!error && hint && (
        <span id={hintId} className={styles.hint}>
          {hint}
        </span>
      )}

      {children({ describedBy })}

      {error && (
        <span id={errorId} role="alert" className={styles.error}>
          {error}
        </span>
      )}
    </div>
  );
};
