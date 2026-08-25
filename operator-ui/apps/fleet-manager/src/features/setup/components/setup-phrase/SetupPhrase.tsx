import { Banner, Button } from '@operator-ui/common-ui';
import { useShowMnemonic } from '@/shared/api/hooks/use-show-mnemonic/useShowMnemonic';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './SetupPhrase.module.css';

interface SetupPhraseProps {
  onSaved: () => void;
}

export const SetupPhrase = ({ onSaved }: SetupPhraseProps) => {
  const showMnemonic = useShowMnemonic();

  const handleReveal = () => {
    showMnemonic.mutate();
  };

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Record your recovery phrase</h1>

      <Banner variant="error">
        These twelve words are the entire fleet's identity and its only backup. Write them down
        offline before continuing — they are never stored in the browser, and losing them loses
        every seat's guardian identity.
      </Banner>
      {showMnemonic.isSuccess ? (
        <div className={styles.phraseBox}>{showMnemonic.data.mnemonic}</div>
      ) : (
        <Button onClick={handleReveal} loading={showMnemonic.isPending}>
          Reveal phrase
        </Button>
      )}
      {showMnemonic.isError ? (
        <span className={styles.error}>{describeActionError(showMnemonic.error)}</span>
      ) : null}

      <Button disabled={!showMnemonic.isSuccess} onClick={onSaved}>
        I've written it down — continue
      </Button>
    </div>
  );
};
