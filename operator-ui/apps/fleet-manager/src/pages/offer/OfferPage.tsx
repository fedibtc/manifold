import { Banner } from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
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
        <h1 className={styles.heading}>Your offer</h1>

        <p className={styles.intro}>
          One price is the whole offer. It is the gross amount a federation initiator pays for a
          seat, before mint and Lightning fees.
        </p>
      </div>

      {/* The field is seeded from the stored price, so the form is a claim about
          the offer and renders through the surface: never before the daemon has
          answered, and under a staleness marker when a refresh has failed. */}
      <QuerySurface disposition={disposition} onRetry={retry}>
        <Banner variant="info">
          Leave the field blank to stop selling seats. A price of 0 keeps the fleet advertised and
          gives seats away free.
        </Banner>

        <form className={styles.form} onSubmit={onSubmit}>
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
