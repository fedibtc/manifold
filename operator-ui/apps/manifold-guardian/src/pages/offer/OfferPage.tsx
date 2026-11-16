import { Banner } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import { DataBudgetForm } from '@/features/offer/components/data-budget-form/DataBudgetForm';
import { SeatCapacityForm } from '@/features/offer/components/seat-capacity-form/SeatCapacityForm';
import { useOfferForm } from '@/features/offer/hooks/use-offer-form/useOfferForm';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import styles from './OfferPage.module.css';

export const OfferPage = () => {
  const { priceSats, onPriceChange, onSubmit, error, isPending, canSubmit, disposition, retry } =
    useOfferForm();

  const handlePriceChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    onPriceChange(event.target.value);
  };

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Seat price</h1>

        <p className={styles.intro}>
          One price is the whole offer. It is what a Federation Ambassador pays for a seat. Network
          fees apply.
        </p>
      </div>

      <SeatCapacityForm />

      <DataBudgetForm />

      <QuerySurface disposition={disposition} onRetry={retry}>
        <form className={styles.form} onSubmit={onSubmit}>
          <label className={styles.label} htmlFor="price-sats">
            Price per seat (sats)
          </label>

          <input
            id="price-sats"
            className={styles.input}
            inputMode="numeric"
            autoComplete="off"
            value={priceSats}
            onChange={handlePriceChange}
            aria-describedby="price-sats-hint"
          />

          <em id="price-sats-hint" className={styles.hint}>
            We suggest the equivalent of $2–$3. A lower price fills your seats sooner.
          </em>

          <div className={styles.priceNote}>
            <Banner variant="info">
              Leave the price blank to stop selling seats. A price of zero keeps this host
              advertised and gives seats away free.
            </Banner>
          </div>

          {error ? <span className={styles.error}>{error}</span> : null}

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
