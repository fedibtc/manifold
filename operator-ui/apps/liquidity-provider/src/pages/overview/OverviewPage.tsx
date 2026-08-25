import { Banner, StatCard, useQueryDisposition } from '@operator-ui/common-ui';
import type { HealthStatus } from '@operator-ui/types';
import { useAdvertisementState } from '@/features/advertisement/hooks/use-advertisement-state/useAdvertisementState';
import { useAllocations } from '@/features/allocations/api/hooks/use-allocations/useAllocations';
import { useFunds } from '@/features/funds/api/hooks/use-funds/useFunds';
import { useWalletOperations } from '@/features/funds/api/hooks/use-wallet-operations/useWalletOperations';
import { useSystemHealth } from '@/features/overview/api/hooks/use-system-health/useSystemHealth';
import { ActivityTable } from '@/features/overview/components/activity-table/ActivityTable';
import { AttentionList } from '@/features/overview/components/attention-list/AttentionList';
import { QuickActions } from '@/features/overview/components/quick-actions/QuickActions';
import { deriveOverview, type OverviewTile } from '@/features/overview/utils/derive';
import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import styles from './OverviewPage.module.css';

type BannerVariant = 'info' | 'warn' | 'error' | 'success';

const toneVariant: Record<HealthStatus, BannerVariant> = {
  healthy: 'success',
  warning: 'warn',
  unhealthy: 'error',
  unknown: 'info'
};

const nowSeconds = (): number => Math.floor(Date.now() / 1000);

const renderTile = (tile: OverviewTile) => (
  <StatCard
    key={tile.key}
    label={tile.label}
    value={tile.value}
    chip={tile.hint ?? undefined}
    chipTone={tile.chipTone ?? undefined}
  />
);

export const OverviewPage = () => {
  const setup = useSetupState();
  const funds = useFunds();
  const walletOperations = useWalletOperations();
  const advertisement = useAdvertisementState();
  const allocations = useAllocations();
  const health = useSystemHealth();

  // Six reads, and the page used to pass only `.data` from each into the
  // formatter and never look at `isError`. Missing values print an em dash, so
  // a failed read and a loading one looked identical — and `deriveStatus`
  // answers "All systems operational" whenever one read holds data and no
  // attention item fires, which is how this screen reported a dead daemon as
  // healthy. The disposition is read over all six: the page claims nothing
  // before they answer, offers a retry when they fail, and keeps the last known
  // figures under a staleness marker when a refresh fails over them.
  const { disposition, retry } = useQueryDisposition([
    setup,
    funds,
    walletOperations,
    advertisement,
    allocations,
    health
  ]);

  const model = deriveOverview({
    setup: setup.data,
    funds: funds.data,
    advertisement: advertisement.data,
    allocations: allocations.data?.allocations.items,
    walletOperations: walletOperations.data?.operations.items,
    health: health.data,
    now: nowSeconds()
  });

  const updatedStamp = model.updatedLabel ? (
    <span className={styles.updated}>{model.updatedLabel}</span>
  ) : undefined;

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Overview</h1>

      <QuerySurface disposition={disposition} onRetry={retry}>
        <Banner
          variant={toneVariant[model.status.tone]}
          title={model.status.headline}
          action={updatedStamp}
        >
          {model.status.subline}
        </Banner>

        <AttentionList items={model.attention} />

        <div className={styles.tileGrid}>{model.tiles.map(renderTile)}</div>

        <QuickActions />

        <ActivityTable rows={model.activity} />
      </QuerySurface>
    </div>
  );
};
