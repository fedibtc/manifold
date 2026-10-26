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
    <SectionCard title="Automatic updates">
      <div className={styles.root}>
        <p>
          This host can install new signed releases of Manifold Fedimint Guardian on its own as soon
          as they are published.
        </p>

        <QuerySurface disposition={disposition} onRetry={retry}>
          <CheckboxField
            label="Install updates automatically"
            checked={enabled}
            onChange={handleChange}
            disabled={setAutoUpdate.isPending}
          />

          {enabled ? null : (
            <Banner variant="warn">
              Automatic updates are off. Install new releases yourself so this host keeps serving
              its federations.
            </Banner>
          )}

          {error ? <span className={styles.error}>{error}</span> : null}
        </QuerySurface>
      </div>
    </SectionCard>
  );
};
