import { type FormEvent, useState } from 'react';
import { useOffer } from '@/shared/api/hooks/use-offer/useOffer';
import { useSetPrice } from '@/shared/api/hooks/use-set-price/useSetPrice';
import {
  type QueryDisposition,
  useQueryDisposition
} from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatPriceField, parsePriceField, readOfferPriceMsat } from '@/shared/utils/offerPrice';

export interface OfferForm {
  priceSats: string;
  onPriceChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  /** The form's own two channels only — a rejected entry and a failed write.
   *  A failed *read* belongs to the surface, which is why it is not folded in
   *  here: it used to overwrite the message the operator needed. */
  error: string | null;
  isPending: boolean;
  disposition: QueryDisposition;
  retry: () => void;
  canSubmit: boolean;
}

export const useOfferForm = (): OfferForm => {
  const offer = useOffer();
  const setPrice = useSetPrice();
  const [priceSats, setPriceSats] = useState('');
  const [hasEdited, setHasEdited] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);
  // Guarded setState during render (not an effect) — the compiler forbids setState
  // inside useEffect, so this is the sanctioned "adjusting state on data change" shape.
  //
  // Seeded exactly once, on the first load. Re-seeding whenever `offer.data`
  // changes identity would let a background refetch overwrite what the operator
  // is part-way through typing: the field is their draft from the first render on.
  const [hasSeeded, setHasSeeded] = useState(false);
  if (!hasSeeded && offer.data) {
    setPriceSats(formatPriceField(readOfferPriceMsat(offer.data.plans)));
    setHasSeeded(true);
  }

  const onPriceChange = (value: string) => {
    setPriceSats(value);
    setHasEdited(true);
    setValidationError(null);
  };

  const { disposition, retry } = useQueryDisposition([offer]);

  // Writing a price needs the stored price to have been read once, so that the
  // operator is overwriting a value they were shown. A `stale` surface still
  // holds that value, so a failed *background* refresh must not take the Save
  // control away — only a surface that has never been answered does.
  const hasOffer = disposition.kind === 'content' || disposition.kind === 'stale';
  const canSubmit = hasOffer && hasEdited;

  const onSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!canSubmit) {
      return;
    }
    const parsed = parsePriceField(priceSats);
    if (!parsed.ok) {
      setValidationError(parsed.error);
      return;
    }
    setValidationError(null);
    setPrice.mutate(parsed.priceMsat, { onSuccess: () => setHasEdited(false) });
  };

  return {
    priceSats,
    onPriceChange,
    onSubmit,
    error: validationError ?? (setPrice.isError ? describeActionError(setPrice.error) : null),
    isPending: setPrice.isPending,
    disposition,
    retry,
    canSubmit
  };
};
