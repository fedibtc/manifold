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
          Anyone who has these 12 words controls this fleet and its funds. Write them down on paper
          — never store them digitally or share them.
        </Banner>

        <div className={styles.phraseBox}>{showMnemonic.data.mnemonic}</div>

        <p className={styles.backupNote}>
          These 12 words are your complete backup. You can use them to restore this fleet on a new
          server — but only during setup of the new server, and only after this one is permanently
          shut down.
        </p>

        <p className={styles.backupNote}>
          The phrase is hidden when you leave this page. You can reveal it again later if needed —
          just make sure you're somewhere private each time.
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
        Your 12-word recovery phrase will be shown on screen. Make sure no one can see your screen
        before continuing.
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
