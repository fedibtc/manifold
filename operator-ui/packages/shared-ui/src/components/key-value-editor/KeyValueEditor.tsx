import { Button } from '../button/Button';
import styles from './KeyValueEditor.module.css';

interface KeyValueEditorProps {
  pairs: [string, string][];
  onChange: (pairs: [string, string][]) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
}

export const KeyValueEditor = ({
  pairs,
  onChange,
  keyPlaceholder,
  valuePlaceholder
}: KeyValueEditorProps) => {
  const updatePair = (index: number, position: 0 | 1, next: string) => {
    const updated = pairs.map((pair, i): [string, string] => {
      if (i !== index) {
        return pair;
      }
      const copy: [string, string] = [pair[0], pair[1]];
      copy[position] = next;
      return copy;
    });
    onChange(updated);
  };
  const removePair = (index: number) => {
    onChange(pairs.filter((_, i) => i !== index));
  };
  const addPair = () => {
    onChange([...pairs, ['', '']]);
  };
  return (
    <div className={styles.wrapper}>
      {pairs.map((pair, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: rows are positional, no stable id available
        <div key={index} className={styles.row}>
          <input
            className={styles.input}
            value={pair[0]}
            placeholder={keyPlaceholder}
            onChange={(event) => updatePair(index, 0, event.target.value)}
            aria-label={`Key ${index + 1}`}
          />

          <input
            className={styles.input}
            value={pair[1]}
            placeholder={valuePlaceholder}
            onChange={(event) => updatePair(index, 1, event.target.value)}
            aria-label={`Value ${index + 1}`}
          />

          <button
            type="button"
            className={styles.remove}
            onClick={() => removePair(index)}
            aria-label={`Remove pair ${index + 1}`}
          >
            ×
          </button>
        </div>
      ))}
      <div>
        <Button variant="secondary" size="small" onClick={addPair}>
          Add
        </Button>
      </div>
    </div>
  );
};
