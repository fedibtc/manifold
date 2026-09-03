import { type FormEvent, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { useOffer } from '@/shared/api/hooks/use-offer/useOffer';
import { useSetCapacity } from '@/shared/api/hooks/use-set-capacity/useSetCapacity';
import { useSetPrice } from '@/shared/api/hooks/use-set-price/useSetPrice';
import {
  type QueryDisposition,
  useQueryDisposition
} from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatPriceField, parsePriceField, readOfferPriceMsat } from '@/shared/utils/offerPrice';
import { describeCapacity, formatSeatsField, parseSeatsField } from '@/shared/utils/seatCapacity';

export interface OfferForm {
  priceSats: string;
  onPriceChange: (value: string) => void;
  maxSeats: string;
  onMaxSeatsChange: (value: string) => void;
  /** The stored ceiling as the operator's own words, or `null` before the
   *  daemon has answered. */
  capacityHint: string | null;
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
  const navigate = useNavigate();
  const offer = useOffer();
  const capacity = useCapacity();
  const setPrice = useSetPrice();
  const setCapacity = useSetCapacity();
  const [priceSats, setPriceSats] = useState('');
  const [maxSeats, setMaxSeats] = useState('');
  const [validationError, setValidationError] = useState<string | null>(null);
  // Guarded setState during render (not an effect) — the compiler forbids setState
  // inside useEffect, so this is the sanctioned "adjusting state on data change" shape.
  //
  // Seeded exactly once, on the first load. Re-seeding whenever the data
  // changes identity would let a background refetch overwrite what the operator
  // is part-way through typing: the fields are their draft from the first
  // render on. Each field is seeded off its own read, because the two reads
  // land independently.
  const [hasSeededPrice, setHasSeededPrice] = useState(false);
  if (!hasSeededPrice && offer.data) {
    setPriceSats(formatPriceField(readOfferPriceMsat(offer.data.plans)));
    setHasSeededPrice(true);
  }
  const [hasSeededSeats, setHasSeededSeats] = useState(false);
  if (!hasSeededSeats && capacity.data) {
    setMaxSeats(formatSeatsField(capacity.data.max_seats));
    setHasSeededSeats(true);
  }

  const onPriceChange = (value: string) => {
    setPriceSats(value);
    setValidationError(null);
  };

  const onMaxSeatsChange = (value: string) => {
    setMaxSeats(value);
    setValidationError(null);
  };

  const { disposition, retry } = useQueryDisposition([offer, capacity]);

  // Writing needs the stored values to have been read once, so that the
  // operator is overwriting values they were shown. A `stale` surface still
  // holds them, so a failed *background* refresh must not take the Save
  // control away — only a surface that has never been answered does.
  const canSubmit = disposition.kind === 'content' || disposition.kind === 'stale';

  // The ceiling is written first, so its refusal is the one that explains why
  // nothing moved. A price failure only ever reaches here on its own.
  const readWriteError = (): string | null => {
    if (setCapacity.isError) return describeActionError(setCapacity.error);
    if (setPrice.isError) return describeActionError(setPrice.error);
    return null;
  };

  const onSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!canSubmit) {
      return;
    }
    const parsedSeats = parseSeatsField(maxSeats);
    if (!parsedSeats.ok) {
      setValidationError(parsedSeats.error);
      return;
    }
    const parsedPrice = parsePriceField(priceSats);
    if (!parsedPrice.ok) {
      setValidationError(parsedPrice.error);
      return;
    }
    setValidationError(null);

    // Only what the operator actually changed is written, and the ceiling goes
    // first because it is the guarded one: a refusal there leaves the offer
    // exactly as it was. Writing an unchanged value would otherwise rotate the
    // offer epoch for nothing, invalidating quotes no one asked to invalidate.
    const storedMaxSeats = capacity.data?.max_seats;
    const storedPriceMsat = offer.data ? readOfferPriceMsat(offer.data.plans) : undefined;

    void (async () => {
      if (parsedSeats.maxSeats !== storedMaxSeats) {
        try {
          await setCapacity.mutateAsync(parsedSeats.maxSeats);
        } catch {
          return;
        }
      }
      if (parsedPrice.priceMsat !== storedPriceMsat) {
        try {
          await setPrice.mutateAsync(parsedPrice.priceMsat);
        } catch {
          return;
        }
      }
      navigate('/');
    })();
  };

  return {
    priceSats,
    onPriceChange,
    maxSeats,
    onMaxSeatsChange,
    capacityHint: capacity.data
      ? describeCapacity(capacity.data.max_seats, capacity.data.available_slots)
      : null,
    onSubmit,
    error: validationError ?? readWriteError(),
    isPending: setPrice.isPending || setCapacity.isPending,
    disposition,
    retry,
    canSubmit
  };
};
