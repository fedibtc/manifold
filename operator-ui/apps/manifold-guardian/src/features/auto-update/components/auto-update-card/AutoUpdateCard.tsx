import { Banner, CheckboxField, SectionCard } from '@operator-ui/common-ui';
import { useAutoUpdate } from '@/features/auto-update/api/hooks/use-auto-update/useAutoUpdate';
import { useSetAutoUpdate } from '@/features/auto-update/api/hooks/use-set-auto-update/useSetAutoUpdate';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './AutoUpdateCard.module.css';

export const AutoUpdateCard = () => {
  const autoUpdate = useAutoUpdate();
  const setAutoUpdate = useSetAutoUpdate();
  const { disposition, retry } = useQueryDisposition([autoUpdate]);

  // While a write is in flight the box shows what was asked for, not the value
  // it is replacing.
  const enabled = setAutoUpdate.isPending
    ? setAutoUpdate.variables
    : (autoUpdate.data?.enabled ?? false);
  const error = setAutoUpdate.isError ? describeActionError(setAutoUpdate.error) : null;

  const handleChange = (checked: boolean) => {
    setAutoUpdate.mutate(checked);
  };

  return (
    <SectionCard title="Signed hot updates">
      <div className={styles.root}>
        <p>
          This host installs new releases of Manifold Fedimint Guardian on its own, without an app
          store update. It installs a release only when several independent release keys signed it.
        </p>

        <QuerySurface disposition={disposition} onRetry={retry}>
          <CheckboxField
            label="Install signed hot updates"
            checked={enabled}
            onChange={handleChange}
            disabled={setAutoUpdate.isPending}
          />

          {enabled ? null : (
            <Banner variant="warn">
              Signed hot updates are off. Install new releases from the app store so this host keeps
              serving its federations.
            </Banner>
          )}

          {error ? <span className={styles.error}>{error}</span> : null}
        </QuerySurface>
      </div>
    </SectionCard>
  );
};
