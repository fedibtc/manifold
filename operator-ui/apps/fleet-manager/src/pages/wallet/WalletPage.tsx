import { Link } from 'react-router-dom';
import { FederationTable } from '@/features/wallet/components/federation-table/FederationTable';
import { deriveWallet } from '@/features/wallet/utils/deriveWallet';
import { usePaymentFederations } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { formatSats } from '@/shared/utils/format';
import styles from './WalletPage.module.css';

export const WalletPage = () => {
  const paymentFederations = usePaymentFederations();

  // "No federations accepted yet" is a claim about the fleet, so it may only be
  // made once the daemon has actually answered — an unanswered query is not an
  // empty wallet. This page argued that case by hand before the primitive
  // existed; it now says it the way every other screen does, which also gets it
  // the retry control and the dated staleness marker it never had.
  const { disposition, retry } = useQueryDisposition([paymentFederations]);

  const federations = paymentFederations.data?.federations ?? [];
  const { totalBalanceMsat, isEmpty } = deriveWallet(federations);

  const wallet = isEmpty ? (
    <p className={styles.empty}>No payment federations accepted yet.</p>
  ) : (
    <>
      <p className={styles.summary}>Total balance: {formatSats(totalBalanceMsat)}</p>

      <FederationTable federations={federations} />
    </>
  );

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Wallet</h1>

        <p className={styles.intro}>
          Membership follows the accepted common setup-payment set, so there is nothing to add or
          remove here. Move this money out on <Link to="/payouts">Payouts</Link>.
        </p>
      </div>

      <QuerySurface disposition={disposition} onRetry={retry}>
        {wallet}
      </QuerySurface>
    </div>
  );
};
