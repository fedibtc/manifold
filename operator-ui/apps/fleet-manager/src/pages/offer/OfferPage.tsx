import { Banner } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import { useOfferForm } from '@/features/offer/hooks/use-offer-form/useOfferForm';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import styles from './OfferPage.module.css';

export const OfferPage = () => {
  const {
    priceSats,
    onPriceChange,
    maxSeats,
    onMaxSeatsChange,
    capacityHint,
    onSubmit,
    error,
    isPending,
    canSubmit,
    disposition,
    retry
  } = useOfferForm();

  const handlePriceChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    onPriceChange(event.target.value);
  };

  const handleMaxSeatsChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    onMaxSeatsChange(event.target.value);
  };

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Your offer</h1>

        <p className={styles.intro}>
          How many seats you will carry, and what one costs. The price is the gross amount a
          federation initiator pays for a seat, before mint and Lightning fees.
        </p>
      </div>

      {/* Both fields are seeded from stored state, so the form is a claim about
          the offer and renders through the surface: never before the daemon has
          answered, and under a staleness marker when a refresh has failed. */}
      <QuerySurface disposition={disposition} onRetry={retry}>
        <Banner variant="info">
          Leave the price blank to stop selling seats. A price of 0 keeps the fleet advertised and
          gives seats away free.
        </Banner>

        <form className={styles.form} onSubmit={onSubmit}>
          <label className={styles.label} htmlFor="max-seats">
            Maximum active seats
          </label>

          {capacityHint ? <span className={styles.hint}>{capacityHint}</span> : null}

          <input
            id="max-seats"
            className={styles.input}
            inputMode="numeric"
            value={maxSeats}
            onChange={handleMaxSeatsChange}
          />

          <label className={styles.label} htmlFor="price-sats">
            Price per seat (sats)
          </label>

          <input
            id="price-sats"
            className={styles.input}
            value={priceSats}
            onChange={handlePriceChange}
          />

          {error ? <span className={styles.error}>{error}</span> : null}

          {/* Both writes rotate the offer epoch when the value really changes,
              and that rotation is what a quote's validity is checked against. */}
          <Banner variant="warn">
            Saving a change re-issues your offer. A quote an initiator has been given but not yet
            paid stops being valid, and they will need a new one.
          </Banner>

          <div className={styles.actions}>
            <Link to="/" className={styles.cancel}>
              Cancel
            </Link>

            <button type="submit" className={styles.submit} disabled={isPending || !canSubmit}>
              Save
            </button>
          </div>
        </form>
      </QuerySurface>
    </div>
  );
};
