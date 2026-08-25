import { afterEach, describe, expect, it } from 'vitest';
import { getState, resetState } from '@/mocks/state';
import { dispatch } from '@/mocks/world/verbs';

const restore = () =>
  dispatch({
    OnboardFromBackup: {
      mnemonic:
        'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
      acknowledge_original_host_is_gone: true
    }
  });

afterEach(() => {
  resetState('not-onboarded');
});

describe('OnboardFromBackup', () => {
  it('should report two seat records with one formed by default', () => {
    resetState('not-onboarded');

    expect(restore()).toEqual({ Ok: { onboarded: 'restored', seats: 2, formed: 1 } });
  });

  it('should report two seat records with none formed when configured', () => {
    resetState('not-onboarded');
    getState().restoreResult = 'two-seats-no-formed';

    expect(restore()).toEqual({ Ok: { onboarded: 'restored', seats: 2, formed: 0 } });
  });

  it('should report no seat records when configured', () => {
    resetState('not-onboarded');
    getState().restoreResult = 'no-seats';

    expect(restore()).toEqual({ Ok: { onboarded: 'restored', seats: 0, formed: 0 } });
  });

  it('should still refuse a phrase that is not twelve words', () => {
    resetState('not-onboarded');

    expect(
      dispatch({
        OnboardFromBackup: { mnemonic: 'too short', acknowledge_original_host_is_gone: true }
      })
    ).toEqual({
      Err: { kind: 'invalid_mnemonic', message: 'that is not a valid mnemonic phrase' }
    });
  });

  it('should floor the offer capacity at the recovered seat count', () => {
    resetState('not-onboarded');

    restore();
    const status = dispatch('Onboarding') as { Ok: Record<string, unknown> };

    expect(status.Ok.stage).toBe('holder_authorization');
    expect(status.Ok.recommended_max_seats).toBe(8);
    expect(status.Ok.minimum_max_seats).toBe(2);
  });
});

describe('staged onboarding dispatch', () => {
  const onboardToAuthorizationStage = () => {
    resetState('not-onboarded');
    dispatch({ OnboardAsNew: { if_needed: false } });
  };

  const onboardToOfferStage = () => {
    onboardToAuthorizationStage();
    dispatch('RefreshHolderAuthorizations');
  };

  it('should refuse a fleet verb while the cursor waits for an authorization', () => {
    onboardToAuthorizationStage();

    expect(dispatch('ListSeats')).toEqual({
      Err: {
        kind: 'not_onboarded',
        message:
          'this Fleet Manager has not been onboarded yet: run `admin onboard new` or `admin onboard restore`'
      }
    });
  });

  it('should refuse the initial offer before an authorization is observed', () => {
    onboardToAuthorizationStage();

    const refused = dispatch({
      ConfigureInitialOffer: { max_seats: 3, price_msats: 50_000_000 }
    }) as { Err: { kind: string } };

    expect(refused.Err.kind).toBe('not_onboarded');
  });

  it('should advance to the offer stage when the refresh retains an authorization', () => {
    onboardToAuthorizationStage();

    const refreshed = dispatch('RefreshHolderAuthorizations') as { Ok: Record<string, unknown> };

    expect(refreshed.Ok.stage).toBe('initial_offer');
  });

  it('should stop serving the refresh once the authorization question is settled', () => {
    onboardToOfferStage();

    const refused = dispatch('RefreshHolderAuthorizations') as { Err: { kind: string } };

    expect(refused.Err.kind).toBe('not_onboarded');
  });

  it('should report starting until the fleet opens, then ready', () => {
    onboardToOfferStage();
    dispatch({ ConfigureInitialOffer: { max_seats: 5, price_msats: 50_000_000 } });

    const whileOpening = dispatch('Onboarding') as { Ok: Record<string, unknown> };
    const afterOpen = dispatch('Onboarding') as { Ok: Record<string, unknown> };

    expect(whileOpening.Ok).toMatchObject({ stage: 'complete', runtime: 'starting' });
    expect(afterOpen.Ok).toMatchObject({ stage: 'complete', runtime: 'ready' });
  });

  it('should refuse a fleet verb between completion and the open fleet', () => {
    onboardToOfferStage();
    dispatch({ ConfigureInitialOffer: { max_seats: 5, price_msats: 50_000_000 } });

    expect(dispatch('ListSeats')).toEqual({
      Err: {
        kind: 'other',
        message:
          'this Fleet Manager has completed onboarding and is starting; its fleet is not open yet'
      }
    });
  });

  it('should persist the configured capacity into the offer', () => {
    onboardToOfferStage();
    dispatch({ ConfigureInitialOffer: { max_seats: 5, price_msats: 50_000_000 } });
    dispatch('Onboarding');
    dispatch('Onboarding');

    expect(dispatch('ShowCapacity')).toEqual({ Ok: { max_seats: 5, available_slots: 5 } });
  });

  it('should refuse an initial offer below the restored seat floor', () => {
    resetState('not-onboarded');
    restore();
    dispatch('RefreshHolderAuthorizations');

    const refused = dispatch({
      ConfigureInitialOffer: { max_seats: 0, price_msats: 50_000_000 }
    }) as { Err: { message: string } };

    expect(refused.Err.message).toBe('cannot set max seats to 0; 2 seats are active');
  });

  it('should accept an initial offer at the restored seat floor', () => {
    resetState('not-onboarded');
    restore();
    dispatch('RefreshHolderAuthorizations');

    expect(dispatch({ ConfigureInitialOffer: { max_seats: 2, price_msats: null } })).toEqual({
      Ok: { onboarding: 'complete', max_seats: 2, plans: [] }
    });
  });

  it('should hold the restored floor against SetCapacity after setup completes', () => {
    resetState('not-onboarded');
    restore();
    dispatch('RefreshHolderAuthorizations');
    dispatch({ ConfigureInitialOffer: { max_seats: 2, price_msats: null } });
    dispatch('Onboarding');
    dispatch('Onboarding');

    const refused = dispatch({ SetCapacity: { max_seats: 1 } }) as { Err: { message: string } };

    expect(refused.Err.message).toBe('cannot set max seats to 1; 2 seats are active');
  });

  it('should refuse the settled setup verbs on a running fleet', () => {
    resetState('fresh-fleet');

    const alreadyOnboarded = {
      Err: {
        kind: 'already_onboarded',
        message: 'this Fleet Manager has already been onboarded; a host is set up once'
      }
    };

    expect(dispatch('RefreshHolderAuthorizations')).toEqual(alreadyOnboarded);
    expect(dispatch({ ConfigureInitialOffer: { max_seats: 3, price_msats: 50_000_000 } })).toEqual(
      alreadyOnboarded
    );
  });
});

describe('SetCapacity', () => {
  it('should refuse a ceiling below the active seats and keep the stored one', () => {
    resetState('seats-mixed');

    const refused = dispatch({ SetCapacity: { max_seats: 2 } }) as { Err: { message: string } };

    expect(refused.Err.message).toBe('cannot set max seats to 2; 3 seats are active');
    expect(dispatch('ShowCapacity')).toEqual({ Ok: { max_seats: 3, available_slots: 0 } });
  });

  it('should raise the ceiling and report the freed slots', () => {
    resetState('seats-mixed');

    expect(dispatch({ SetCapacity: { max_seats: 5 } })).toEqual({
      Ok: { max_seats: 5, available_slots: 2 }
    });
    expect(dispatch('ShowCapacity')).toEqual({ Ok: { max_seats: 5, available_slots: 2 } });
  });
});
