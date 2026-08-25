import type { OnboardingNostrStatus, PaymentFederation, Plan } from '@operator-ui/types';
import { readOfferPriceMsat } from '@/shared/utils/offerPrice';

export interface AttentionItem {
  key: string;
  title: string;
  detail: string;
  path: string;
}

export interface OverviewModel {
  tone: 'success' | 'warn';
  headline: string;
  attention: AttentionItem[];
}

export interface OverviewInputs {
  paymentFederations?: PaymentFederation[];
  plans?: Plan[];
  /** Absent while the Onboarding query has not answered. The Overview says nothing
   *  rather than guessing. */
  nostrState?: OnboardingNostrStatus['state'];
}

// ListSeats returns SeatSummary only — no health/phase (that's SeatStatus, a
// per-seat call). Overview deliberately does not N+1-fetch every seat's status
// just to roll up a health count; per-seat health lives on the Seats section.
export const deriveOverview = ({
  paymentFederations = [],
  plans = [],
  nostrState
}: OverviewInputs): OverviewModel => {
  const priceMsat = readOfferPriceMsat(plans);
  const isSellingForMoney = priceMsat !== null && priceMsat > 0;
  const canReceive = paymentFederations.some(
    (federation) => federation.accepted && federation.receivable
  );

  const attention: AttentionItem[] = paymentFederations
    .filter((federation) => federation.accepted && !federation.receivable)
    .map((federation) => ({
      key: federation.federation_id,
      title: 'Payment federation not receiving',
      detail: federation.federation_id,
      path: '/wallet'
    }));

  // A paid offer with nowhere to receive payment advertises seats nobody can
  // buy. The daemon refuses to set a price in an environment with no
  // setup-payment publisher at all, but it cannot tell that every accepted
  // federation has since stopped receiving — which looks identical to a healthy
  // fleet from the operator's side until someone tries to pay.
  if (isSellingForMoney && !canReceive) {
    attention.push({
      key: 'no-receivable-payment-federation',
      title: 'Seats are priced but cannot be paid for',
      detail: 'No accepted payment federation can receive, so no seat can be bought.',
      path: '/offer'
    });
  }

  // These items can now be definite. The daemon used to answer
  // `waiting_for_authorization` both for a fleet nobody had authorized and for
  // one whose relay it had not read, so this could only report what was *not*
  // known. It reports the four states separately instead.
  //
  // `checking` raises nothing. It is the state between daemon start and the
  // first relay read, it clears on its own, and an attention item the operator
  // cannot act on is noise.
  if (nostrState === 'not_observed') {
    attention.push({
      key: 'authorization-not-observed',
      title: 'No holder has authorized this fleet',
      detail: 'Initiators cannot evaluate the fleet until one does. Open Authorization to check.',
      path: '/authorization'
    });
  }

  // A failed read is not the same as an absent authorization, and the operator
  // is the one who can act on it. The item does not carry the daemon's error
  // text: the Authorization page shows that, and this is a list of one-line
  // titles.
  if (nostrState === 'relay_error') {
    attention.push({
      key: 'authorization-relay-error',
      title: 'The relay could not be read',
      detail: 'The fleet may or may not be authorized. Open Authorization for the failure.',
      path: '/authorization'
    });
  }

  return {
    tone: attention.length === 0 ? 'success' : 'warn',
    headline: attention.length === 0 ? 'Advertised and healthy' : 'Needs your attention',
    attention
  };
};
