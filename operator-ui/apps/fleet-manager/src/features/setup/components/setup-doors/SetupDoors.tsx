import { Banner, Button } from '@operator-ui/common-ui';
import { useOnboardAsNew } from '@/features/setup/api/hooks/use-onboard-as-new/useOnboardAsNew';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './SetupDoors.module.css';

interface SetupDoorsProps {
  onNewFleet: () => void;
  onRestore: () => void;
}

export const SetupDoors = ({ onNewFleet, onRestore }: SetupDoorsProps) => {
  const onboardAsNew = useOnboardAsNew();

  const handleNewFleet = () => {
    onboardAsNew.mutate(undefined, { onSuccess: onNewFleet });
  };

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Set up your fleet manager</h1>

        <p className={styles.intro}>
          This host has no identity yet. A fleet manager is set up once — choose which one this is.
        </p>
      </div>

      <div className={styles.doors}>
        <section className={styles.door}>
          <h2 className={styles.doorTitle}>Start a new fleet</h2>

          <p className={styles.doorBody}>
            Generates a fresh recovery phrase and starts with no seats. This is the usual choice.
          </p>

          <Button onClick={handleNewFleet} loading={onboardAsNew.isPending}>
            Start a new fleet
          </Button>
        </section>

        <section className={styles.door}>
          <h2 className={styles.doorTitle}>Recover from your phrase</h2>

          <p className={styles.doorBody}>
            Rebuilds a fleet manager whose original host is gone, from the twelve words you wrote
            down. Its seats come back with it.
          </p>

          <Button variant="secondary" onClick={onRestore}>
            Recover from a phrase
          </Button>
        </section>
      </div>

      <Banner variant="info">
        Recovery is only offered here. Once this host is set up, the choice is made and cannot be
        revisited.
      </Banner>
      {onboardAsNew.isError ? (
        <span className={styles.error}>{describeActionError(onboardAsNew.error)}</span>
      ) : null}
    </div>
  );
};
