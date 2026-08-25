import { Banner, Button } from '@operator-ui/common-ui';
import { type FormEvent, useState } from 'react';
import { useConfigureInitialOffer } from '@/features/setup/api/hooks/use-configure-initial-offer/useConfigureInitialOffer';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { describeActionError } from '@/shared/utils/describeActionError';
import { parsePriceField } from '@/shared/utils/offerPrice';
import styles from './SetupPrice.module.css';

interface SetupPriceProps {
  onDone: () => void;
}

export const SetupPrice = ({ onDone }: SetupPriceProps) => {
  const configureOffer = useConfigureInitialOffer();
  const onboarding = useOnboarding();
  const [priceSats, setPriceSats] = useState('');
  const [maxSeats, setMaxSeats] = useState('0');
  const [validationError, setValidationError] = useState<string | null>(null);
  const [hasSeededMaxSeats, setHasSeededMaxSeats] = useState(false);
  if (!hasSeededMaxSeats && onboarding.data) {
    setMaxSeats(
      String(
        Math.max(onboarding.data.recommended_max_seats ?? 0, onboarding.data.minimum_max_seats ?? 0)
      )
    );
    setHasSeededMaxSeats(true);
  }

  const handlePriceChange = (event: FormEvent<HTMLInputElement>) => {
    setPriceSats(event.currentTarget.value);
    setValidationError(null);
  };

  const handleMaxSeatsChange = (event: FormEvent<HTMLInputElement>) => {
    setMaxSeats(event.currentTarget.value);
    setValidationError(null);
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const parsed = parsePriceField(priceSats);
    if (!parsed.ok) {
      setValidationError(parsed.error);
      return;
    }
    const parsedMaxSeats = Number(maxSeats);
    if (!Number.isInteger(parsedMaxSeats) || parsedMaxSeats < 0 || parsedMaxSeats > 4_294_967_295) {
      setValidationError('Maximum seats must be a whole number from 0 to 4294967295.');
      return;
    }
    setValidationError(null);
    configureOffer.mutate(
      { maxSeats: parsedMaxSeats, priceMsat: parsed.priceMsat },
      { onSuccess: onDone }
    );
  };

  const error =
    validationError ?? (configureOffer.isError ? describeActionError(configureOffer.error) : null);

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Set your price</h1>

        <p className={styles.intro}>
          One price is the whole offer: the gross amount an initiator pays for a seat, before mint
          and Lightning fees.
        </p>
      </div>

      <Banner variant="info">
        Leave this blank to finish setup without selling seats. You can set a price any time from
        the Overview.
      </Banner>

      <form className={styles.form} onSubmit={handleSubmit}>
        <label className={styles.label} htmlFor="setup-max-seats">
          Maximum active seats
        </label>

        <input
          id="setup-max-seats"
          className={styles.input}
          inputMode="numeric"
          value={maxSeats}
          onChange={handleMaxSeatsChange}
        />

        <label className={styles.label} htmlFor="setup-price-sats">
          Price per seat (sats)
        </label>

        <input
          id="setup-price-sats"
          className={styles.input}
          value={priceSats}
          onChange={handlePriceChange}
        />
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
