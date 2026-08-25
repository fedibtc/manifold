import type { ChangeEvent } from 'react';
import styles from './VerbErrorList.module.css';

/** Sentinel for "serve this verb normally". A select needs a value for its
 *  cleared state, and the absence of a forced error has no code of its own. */
const NO_ERROR = '';

export interface VerbErrorListProps {
  verbs: readonly string[];
  codes: readonly string[];
  active: Readonly<Record<string, string>>;
  onChange: (verb: string, code: string | null) => void;
}

export const VerbErrorList = ({ verbs, codes, active, onChange }: VerbErrorListProps) => {
  const handleChange = (event: ChangeEvent<HTMLSelectElement>) => {
    const { value } = event.target;
    onChange(event.target.name, value === NO_ERROR ? null : value);
  };

  return (
    <ul className={styles.list}>
      {verbs.map((verb) => (
        <li className={styles.row} key={verb}>
          <label className={styles.verb} htmlFor={`verb-error-${verb}`}>
            {verb}
          </label>

          <select
            className={styles.select}
            id={`verb-error-${verb}`}
            name={verb}
            value={active[verb] ?? NO_ERROR}
            onChange={handleChange}
          >
            <option value={NO_ERROR}>no error</option>

            {codes.map((code) => (
              <option key={code} value={code}>
                {code}
              </option>
            ))}
          </select>
        </li>
      ))}
    </ul>
  );
};
