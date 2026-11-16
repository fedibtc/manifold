import { RadioGroupField, TextInput } from '@operator-ui/common-ui';
import {
  DATA_BUDGET_LABEL,
  DATA_CAP_OPTIONS,
  DATA_CAP_QUESTION,
  type DataCapChoice
} from '@/shared/utils/dataBudget';
import styles from './MonthlyDataField.module.css';

interface MonthlyDataFieldProps {
  choice: DataCapChoice | null;
  onChoiceChange: (choice: DataCapChoice) => void;
  gb: string;
  onGbChange: (gb: string) => void;
  /** Under the question: what the answer does. */
  hint: string;
  /** Under the amount, once the plan has a limit. */
  gbHint: string;
  choiceError?: string;
  gbError?: string;
  /** What a saved amount covers, such as "500 GB covers 8 seats." */
  coverage?: string | null;
  disabled?: boolean;
}

/** Whether the host's internet has a monthly data cap, and if so how much of
 *  it this guardian may use. The amount appears only for a capped plan. */
export const MonthlyDataField = ({
  choice,
  onChoiceChange,
  gb,
  onGbChange,
  hint,
  gbHint,
  choiceError,
  gbError,
  coverage,
  disabled = false
}: MonthlyDataFieldProps) => (
  <div className={styles.root}>
    <RadioGroupField
      label={DATA_CAP_QUESTION}
      hint={hint}
      options={DATA_CAP_OPTIONS}
      value={choice}
      onChange={onChoiceChange}
      error={choiceError}
      disabled={disabled}
    />

    {choice === 'limited' ? (
      <div className={styles.amount}>
        <TextInput
          label={DATA_BUDGET_LABEL}
          hint={gbHint}
          value={gb}
          onChange={onGbChange}
          error={gbError}
          disabled={disabled}
        />

        {coverage ? <p className={styles.coverage}>{coverage}</p> : null}
      </div>
    ) : null}
  </div>
);
