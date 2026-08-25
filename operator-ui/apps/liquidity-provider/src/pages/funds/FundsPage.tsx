import { Banner, StatCard } from '@operator-ui/common-ui';
import { useState } from 'react';
import { useFunds } from '@/features/funds/api/hooks/use-funds/useFunds';
import { useWalletOperations } from '@/features/funds/api/hooks/use-wallet-operations/useWalletOperations';
import { BalanceBreakdown } from '@/features/funds/components/balance-breakdown/BalanceBreakdown';
import { FundsActions } from '@/features/funds/components/funds-actions/FundsActions';
import { ManualReviewPanel } from '@/features/funds/components/manual-review-panel/ManualReviewPanel';
import { SourcesTable } from '@/features/funds/components/sources-table/SourcesTable';
import { WalletOperationsTable } from '@/features/funds/components/wallet-operations-table/WalletOperationsTable';
import { deriveFunds } from '@/features/funds/utils/deriveFunds';
import { formatSats } from '@/shared/utils/format';
import styles from './FundsPage.module.css';

const PageHeader = () => (
  <header className={styles.head}>
    <h1 className={styles.heading}>Funds</h1>

    <p className={styles.sub}>Hot wallet balance and on-chain operations.</p>
  </header>
);

export const FundsPage = () => {
  const funds = useFunds();
  const walletOperations = useWalletOperations();
  // The operation the operator opened a resolution for. Held here rather than
  // in the table because the panel is a page-level decision, and the table is
  // rendered from a summary that cannot supply what the panel needs.
  const [reviewing, setReviewing] = useState<string | null>(null);

  if (!funds.data) {
    if (funds.isError) {
      const message =
        funds.error instanceof Error ? funds.error.message : 'The funds service is unavailable.';
      return (
        <div className={styles.page}>
          <PageHeader />

          <Banner variant="error" title="Couldn't load funds">
            {message}
          </Banner>
        </div>
      );
    }

    return (
      <div className={styles.page}>
        <PageHeader />

        <p className={styles.loading}>Loading funds…</p>
      </div>
    );
  }

  const model = deriveFunds(funds.data);
  const operations = walletOperations.data?.operations.items ?? [];

  return (
    <div className={styles.page}>
      <PageHeader />

      {funds.isError && (
        <Banner variant="warn" title="Showing last-known data">
          {funds.dataUpdatedAt
            ? `Retrying the connection — last updated ${new Date(funds.dataUpdatedAt).toLocaleTimeString()}.`
            : 'Retrying the connection.'}
        </Banner>
      )}

      {model.banner && (
        <Banner variant={model.banner.variant} title={model.banner.title}>
          {model.banner.message}
        </Banner>
      )}

      <div className={styles.metricRow}>
        <StatCard
          label="Available to allocate"
          value={formatSats(model.availableBalance)}
          chip={model.balanceChip.label}
          chipTone={model.balanceChip.tone}
        />
      </div>

      <BalanceBreakdown rows={model.balanceRows} />

      <div className={styles.actions}>
        <FundsActions />
      </div>

      <SourcesTable rows={model.sourceRows} />

      <WalletOperationsTable operations={operations} onResolve={setReviewing} />

      {reviewing && (
        <ManualReviewPanel operationId={reviewing} onClose={() => setReviewing(null)} />
      )}
    </div>
  );
};
