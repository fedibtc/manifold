import { Banner, Button } from '@operator-ui/common-ui';
import { type FormEvent, useState } from 'react';
import { useConfigureInitialOffer } from '@/features/setup/api/hooks/use-configure-initial-offer/useConfigureInitialOffer';
import { MonthlyDataField } from '@/shared/components/monthly-data-field/MonthlyDataField';
import {
  DATA_AMOUNT_HINT,
  DATA_BUDGET_EXPLAINER,
  DATA_BUDGET_EXPLAINER_WITHOUT_BADGE,
  type DataCapChoice,
  parseDataBudget
} from '@/shared/utils/dataBudget';
import { describeActionError } from '@/shared/utils/describeActionError';
import { parsePriceField } from '@/shared/utils/offerPrice';
import styles from './SetupPrice.module.css';

const NOT_SURE = 'Not sure? Check your internet plan or bill.';

const NO_PRICE = { ok: true, priceMsat: null } as const;

interface SetupPriceProps {
  /** False for a host set up without a badge: it takes seats only through
   *  guardian codes, which are free, so it has no price to set. */
  withPrice: boolean;
  onDone: () => void;
}

export const SetupPrice = ({ withPrice, onDone }: SetupPriceProps) => {
  const configureOffer = useConfigureInitialOffer();
  const [priceSats, setPriceSats] = useState('');
  const [dataCap, setDataCap] = useState<DataCapChoice | null>(null);
  const [dataGb, setDataGb] = useState('');
  const [validationError, setValidationError] = useState<string | null>(null);
  const [dataErrors, setDataErrors] = useState<{ choice?: string; gb?: string }>({});

  const handlePriceChange = (event: FormEvent<HTMLInputElement>) => {
    setPriceSats(event.currentTarget.value);
    setValidationError(null);
  };

  const handleDataCapChange = (choice: DataCapChoice) => {
    setDataCap(choice);
    setDataErrors({});
  };

  const handleDataGbChange = (gb: string) => {
    setDataGb(gb);
    setDataErrors({});
  };

  // The seat limit starts automatic: Fedi's policy and this host's memory set
  // it. The operator can set their own limit later, on the Overview.
  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const parsed = withPrice ? parsePriceField(priceSats) : NO_PRICE;
    const budget = parseDataBudget(dataCap, dataGb);
    setValidationError(parsed.ok ? null : parsed.error);
    setDataErrors(budget.ok ? {} : { [budget.field]: budget.error });
    if (!parsed.ok || !budget.ok) return;
    configureOffer.mutate(
      { maxSeats: null, priceMsat: parsed.priceMsat, monthlyDataBudget: budget.budget },
      { onSuccess: onDone }
    );
  };

  const error =
    validationError ?? (configureOffer.isError ? describeActionError(configureOffer.error) : null);

  return (
    <div className={styles.root}>
      {withPrice ? (
        <>
          <div className={styles.head}>
            <h1 className={styles.heading}>Set your price</h1>

            <p className={styles.intro}>
              One price is the whole offer: the gross amount a Federation Ambassador pays for a
              seat, before mint and Lightning fees.
            </p>
          </div>

          <Banner variant="info">
            Leave this blank to finish setup without selling seats. You can set a price any time
            from the Overview.
          </Banner>
        </>
      ) : (
        <div className={styles.head}>
          <h1 className={styles.heading}>Set your monthly data</h1>

          <p className={styles.intro}>
            Without a badge there is no seat price: guardian codes are always free. One question is
            left before this host goes live.
          </p>
        </div>
      )}

      <form className={styles.form} onSubmit={handleSubmit}>
        {withPrice ? (
          <>
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
          </>
        ) : null}

        {error ? <span className={styles.error}>{error}</span> : null}

        <div className={withPrice ? styles.data : undefined}>
          <MonthlyDataField
            choice={dataCap}
            onChoiceChange={handleDataCapChange}
            gb={dataGb}
            onGbChange={handleDataGbChange}
            hint={`${withPrice ? DATA_BUDGET_EXPLAINER : DATA_BUDGET_EXPLAINER_WITHOUT_BADGE} ${NOT_SURE}`}
            gbHint={DATA_AMOUNT_HINT}
            choiceError={dataErrors.choice}
            gbError={dataErrors.gb}
            disabled={configureOffer.isPending}
          />
        </div>

        <div className={styles.actions}>
          <Button type="submit" loading={configureOffer.isPending}>
            Finish setup
          </Button>
        </div>
      </form>
    </div>
  );
};
