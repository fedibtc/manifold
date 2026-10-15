import { Banner, Button } from '@operator-ui/common-ui';
import { type FormEvent, useState } from 'react';
import { useConfigureInitialOffer } from '@/features/setup/api/hooks/use-configure-initial-offer/useConfigureInitialOffer';
import { describeActionError } from '@/shared/utils/describeActionError';
import { parsePriceField } from '@/shared/utils/offerPrice';
import styles from './SetupPrice.module.css';

interface SetupPriceProps {
  onDone: () => void;
}

export const SetupPrice = ({ onDone }: SetupPriceProps) => {
  const configureOffer = useConfigureInitialOffer();
  const [priceSats, setPriceSats] = useState('');
  const [validationError, setValidationError] = useState<string | null>(null);

  const handlePriceChange = (event: FormEvent<HTMLInputElement>) => {
    setPriceSats(event.currentTarget.value);
    setValidationError(null);
  };

  // The seat limit starts automatic: Fedi's policy and this host's memory set
  // it. The operator can set their own limit later, on the Overview.
  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const parsed = parsePriceField(priceSats);
    if (!parsed.ok) {
      setValidationError(parsed.error);
      return;
    }
    setValidationError(null);
    configureOffer.mutate({ maxSeats: null, priceMsat: parsed.priceMsat }, { onSuccess: onDone });
  };

  const error =
    validationError ?? (configureOffer.isError ? describeActionError(configureOffer.error) : null);

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Set your price</h1>

        <p className={styles.intro}>
          One price is the whole offer: the gross amount a Federation Ambassador pays for a seat,
          before mint and Lightning fees.
        </p>
      </div>

      <Banner variant="info">
        Leave this blank to finish setup without selling seats. You can set a price any time from
        the Overview.
      </Banner>

      <form className={styles.form} onSubmit={handleSubmit}>
        <label className={styles.label} htmlFor="setup-price-sats">
          Price per seat (sats)
        </label>

        <input
          id="setup-price-sats"
          className={styles.input}
          inputMode="numeric"
          autoComplete="off"
          value={priceSats}
          onChange={handlePriceChange}
          aria-describedby="setup-price-sats-hint"
        />

        <em id="setup-price-sats-hint" className={styles.hint}>
          We suggest the equivalent of $2–$3. A lower price fills your seats sooner.
        </em>

        {error ? <span className={styles.error}>{error}</span> : null}

        <div className={styles.actions}>
          <Button type="submit" loading={configureOffer.isPending}>
            Finish setup
          </Button>
        </div>
      </form>
    </div>
  );
};
