import { Banner, Button, SectionCard, TextInput } from '@operator-ui/common-ui';
import { usePayoutDestinationForm } from '@/features/payouts/components/payout-destination-card/usePayoutDestinationForm';
import styles from './PayoutDestinationCard.module.css';

interface PayoutDestinationCardProps {
  /** The stored destination, or `null` when the fleet has none. */
  destination: string | null;
}

/**
 * Step zero of every payout. It sits above both revenue sections because the
 * daemon refuses a sweep outright while no destination is stored
 * (crates/fman/core/src/fleet.rs:1130) — an operator should read that ordering
 * off the screen rather than discover it through a refusal.
 */
export const PayoutDestinationCard = ({ destination }: PayoutDestinationCardProps) => {
  const form = usePayoutDestinationForm(destination);

  return (
    <SectionCard title="Payout destination">
      <div className={styles.root}>
        {destination === null ? (
          <Banner variant="warn" title="Add a payout address to withdraw">
            You can still collect guardian fees below — that step doesn't need one.
          </Banner>
        ) : (
          <p className={styles.current}>
            Revenue leaves to <span className={styles.value}>{destination}</span>
          </p>
        )}

        <form className={styles.form} onSubmit={form.onSubmit}>
          <TextInput
            label="Lightning address or LNURL-pay"
            value={form.value}
            onChange={form.onChange}
            disabled={form.isPending}
            placeholder="operator@example.com"
            hint="Enter a reusable Lightning address (like operator@example.com). One-time invoices won't work — this address gets used for every withdrawal."
            error={form.error ?? undefined}
          />

          <div className={styles.actions}>
            <Button type="submit" disabled={!form.canSave} loading={form.isPending}>
              Save destination
            </Button>

            {destination !== null && (
              <Button variant="secondary" disabled={form.isPending} onClick={form.onClear}>
                Clear
              </Button>
            )}
          </div>
        </form>
      </div>
    </SectionCard>
  );
};
