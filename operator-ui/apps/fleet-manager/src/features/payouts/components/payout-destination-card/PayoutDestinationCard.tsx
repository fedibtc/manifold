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
          <Banner variant="warn" title="No payout destination">
            Sweeps are refused until one is set. Collecting guardian fees out of the pool still
            works — that moves money inside the fleet, not out of it.
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
            hint="The daemon pays this over LNURL. A bolt11 invoice is not accepted — it is single-use, and every sweep reuses this destination."
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
