import { Banner } from '@operator-ui/common-ui';
import { useEffect } from 'react';
import { Link } from 'react-router-dom';
import { useShowMnemonic } from '@/shared/api/hooks/use-show-mnemonic/useShowMnemonic';
import styles from './BackupPhrasePage.module.css';

export const BackupPhrasePage = () => {
  const showMnemonic = useShowMnemonic();
  const { reset } = showMnemonic;

  // This screen owns the phrase, so it also disposes of it: `reset()` drops the
  // mutation's own copy of the result, and gcTime: 0 collects the cache entry
  // behind it. `reset` is bound once per mutation observer, so this runs on
  // unmount and never mid-screen.
  useEffect(() => reset, [reset]);

  const handleReveal = () => {
    showMnemonic.mutate();
  };

  if (showMnemonic.isSuccess) {
    return (
      <div className={styles.root}>
        <h1 className={styles.heading}>Recovery phrase</h1>

        <Banner variant="error">
          This is the entire fleet's identity — anyone who has it owns the fleet. Never store it
          digitally or share it; write it down now.
        </Banner>

        <div className={styles.phraseBox}>{showMnemonic.data.mnemonic}</div>

        <p className={styles.backupNote}>
          These twelve words are a complete backup. Restoring them onto a new host recovers this
          fleet's seats — but only during that host's setup, and only once the guardians running
          here are permanently offline.
        </p>

        <p className={styles.backupNote}>
          Leaving this page hides the phrase. You can come back and reveal it again, so a missed
          word is not lost — but each reveal is another chance for someone to read it over your
          shoulder.
        </p>

        <Link to="/backup" className={styles.done}>
          Done
        </Link>
      </div>
    );
  }

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Reveal recovery phrase</h1>

      <p className={styles.intro}>
        This fetches and displays the fleet's 12-word root mnemonic. Nothing is fetched until you
        confirm, and leaving the page hides it again. Reveal it only where nobody can read it.
      </p>

      <div className={styles.actions}>
        <Link to="/backup" className={styles.cancel}>
          Cancel
        </Link>

        <button
          type="button"
          className={styles.reveal}
          disabled={showMnemonic.isPending}
          onClick={handleReveal}
        >
          Reveal phrase
        </button>
      </div>
    </div>
  );
};
