import styles from './Stepper.module.css';

interface StepperProps {
  steps: string[];
  current: number;
  completed?: number[];
}

type StepState = 'current' | 'completed' | 'upcoming';

export const Stepper = ({ steps, current, completed = [] }: StepperProps) => {
  const stepState = (index: number): StepState => {
    if (index === current) {
      return 'current';
    }
    if (completed.includes(index)) {
      return 'completed';
    }
    return 'upcoming';
  };
  return (
    <ol className={styles.root}>
      {steps.map((label, index) => {
        const state = stepState(index);
        return (
          <li key={label} aria-current={state === 'current' ? 'step' : undefined}>
            <span data-state={state} className={styles.step}>
              {label}
            </span>

            <span className={styles.visuallyHidden}>{state}</span>
          </li>
        );
      })}
    </ol>
  );
};
