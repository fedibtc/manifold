import type { MonthlyDataBudget } from '@operator-ui/types';
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
          'this Manifold Fedimint Guardian has not been set up yet: run `admin onboard new` or `admin onboard restore`'
      }
    });
  });

  it('should refuse the initial offer before an authorization is observed', () => {
    onboardToAuthorizationStage();

    const refused = dispatch({
      ConfigureInitialOffer: {
        max_seats: 3,
        price_msats: 50_000_000,
        monthly_data_budget: 'unlimited'
      }
    }) as { Err: { kind: string } };

    expect(refused.Err.kind).toBe('not_onboarded');
  });

  it('should advance to the offer stage when the refresh retains an authorization', () => {
    onboardToAuthorizationStage();

    const refreshed = dispatch('RefreshHolderAuthorizations') as { Ok: Record<string, unknown> };

    expect(refreshed.Ok.stage).toBe('initial_offer');
  });

  it('should advance to the offer stage on a skip, with no authorization retained', () => {
    onboardToAuthorizationStage();
    getState().relayAuthorization = 'absent';

    const skipped = dispatch('SkipHolderAuthorization') as { Ok: Record<string, unknown> };
    const retried = dispatch('SkipHolderAuthorization') as { Ok: Record<string, unknown> };

    expect(skipped.Ok.stage).toBe('initial_offer');
    expect(retried.Ok.stage).toBe('initial_offer');
    expect((skipped.Ok.nostr as { state: string }).state).not.toBe('authorization_observed');
  });

  it('should refuse the skip where the network has no badgeless path', () => {
    onboardToAuthorizationStage();
    getState().onboarding.badgeless_mg = false;

    const refused = dispatch('SkipHolderAuthorization') as { Err: { message: string } };

    expect(refused.Err.message).toMatch(/without a badge/);
    expect((dispatch('Onboarding') as { Ok: { stage: string } }).Ok.stage).toBe(
      'holder_authorization'
    );
  });

  it('should stop serving the refresh once the authorization question is settled', () => {
    onboardToOfferStage();

    const refused = dispatch('RefreshHolderAuthorizations') as { Err: { kind: string } };

    expect(refused.Err.kind).toBe('not_onboarded');
  });

  it('should report starting until the fleet opens, then ready', () => {
    onboardToOfferStage();
    dispatch({
      ConfigureInitialOffer: {
        max_seats: 5,
        price_msats: 50_000_000,
        monthly_data_budget: 'unlimited'
      }
    });

    const whileOpening = dispatch('Onboarding') as { Ok: Record<string, unknown> };
    const afterOpen = dispatch('Onboarding') as { Ok: Record<string, unknown> };

    expect(whileOpening.Ok).toMatchObject({ stage: 'complete', runtime: 'starting' });
    expect(afterOpen.Ok).toMatchObject({ stage: 'complete', runtime: 'ready' });
  });

  it('should refuse a fleet verb between completion and the open fleet', () => {
    onboardToOfferStage();
    dispatch({
      ConfigureInitialOffer: {
        max_seats: 5,
        price_msats: 50_000_000,
        monthly_data_budget: 'unlimited'
      }
    });

    expect(dispatch('ListSeats')).toEqual({
      Err: {
        kind: 'other',
        message:
          'this Manifold Fedimint Guardian has completed setup and is starting; its guardians are not open yet'
      }
    });
  });

  it('should persist the configured capacity into the offer', () => {
    onboardToOfferStage();
    dispatch({
      ConfigureInitialOffer: {
        max_seats: 5,
        price_msats: 50_000_000,
        monthly_data_budget: 'unlimited'
      }
    });
    dispatch('Onboarding');
    dispatch('Onboarding');

    expect(dispatch('ShowCapacity')).toEqual({
      Ok: {
        max_seats: 5,
        memory_max_seats: 9,
        policy_max_seats: 12,
        cpu_max_seats: null,
        cpu_benchmark: {
          cpu_count: 16,
          iterations_per_sec: 12_600,
          single_core_iterations_per_sec: 810
        },
        disk_max_seats: null,
        monthly_data_budget: 'unlimited',
        seat_monthly_data_gb: 65,
        data_max_seats: null,
        available_slots: 5
      }
    });
  });

  it('should keep a budget given during setup', () => {
    onboardToOfferStage();
    dispatch({
      ConfigureInitialOffer: { max_seats: null, price_msats: null, monthly_data_budget: '500' }
    });
    dispatch('Onboarding');
    dispatch('Onboarding');

    expect(dispatch('ShowCapacity')).toMatchObject({
      Ok: { monthly_data_budget: '500', data_max_seats: 7 }
    });
  });

  it('should refuse an invalid budget at setup without completing it', () => {
    onboardToOfferStage();

    const refused = dispatch({
      ConfigureInitialOffer: { max_seats: null, price_msats: null, monthly_data_budget: '0' }
    }) as { Err: { message: string } };

    expect(refused.Err.message).toMatch(/whole number of GB from 1/);
    expect(getState().onboarding.stage).not.toBe('complete');
  });

  it('should refuse an initial offer below the restored seat floor', () => {
    resetState('not-onboarded');
    restore();
    dispatch('RefreshHolderAuthorizations');

    const refused = dispatch({
      ConfigureInitialOffer: {
        max_seats: 0,
        price_msats: 50_000_000,
        monthly_data_budget: 'unlimited'
      }
    }) as { Err: { message: string } };

    expect(refused.Err.message).toBe('cannot set max seats to 0; 2 seats are active');
  });

  it('should accept an initial offer at the restored seat floor', () => {
    resetState('not-onboarded');
    restore();
    dispatch('RefreshHolderAuthorizations');

    expect(
      dispatch({
        ConfigureInitialOffer: { max_seats: 2, price_msats: null, monthly_data_budget: 'unlimited' }
      })
    ).toEqual({
      Ok: {
        onboarding: 'complete',
        max_seats: 2,
        monthly_data_budget: 'unlimited',
        plans: []
      }
    });
  });

  it('should hold the restored floor against SetCapacity after setup completes', () => {
    resetState('not-onboarded');
    restore();
    dispatch('RefreshHolderAuthorizations');
    dispatch({
      ConfigureInitialOffer: { max_seats: 2, price_msats: null, monthly_data_budget: 'unlimited' }
    });
    dispatch('Onboarding');
    dispatch('Onboarding');

    const refused = dispatch({ SetCapacity: { max_seats: 1 } }) as { Err: { message: string } };

    expect(refused.Err.message).toBe('cannot set max seats to 1; 2 seats are active');
  });

  it('should allow authorization refresh but refuse initial offer setup on a running fleet', () => {
    resetState('fresh-fleet');

    const alreadyOnboarded = {
      Err: {
        kind: 'already_onboarded',
        message: 'this Manifold Fedimint Guardian has already been set up; a host is set up once'
      }
    };

    const refreshed = dispatch('RefreshHolderAuthorizations') as { Ok: Record<string, unknown> };
    expect(refreshed.Ok.nostr).toMatchObject({ state: 'authorization_observed' });
    getState().relayAuthorization = 'absent';
    const emptyRefresh = dispatch('RefreshHolderAuthorizations') as { Ok: Record<string, unknown> };
    expect(emptyRefresh.Ok.nostr).toEqual(refreshed.Ok.nostr);
    expect(
      dispatch({
        ConfigureInitialOffer: {
          max_seats: 3,
          price_msats: 50_000_000,
          monthly_data_budget: 'unlimited'
        }
      })
    ).toEqual(alreadyOnboarded);
  });
});

describe('SetCapacity', () => {
  it('should refuse a ceiling below the active seats and keep the stored one', () => {
    resetState('seats-mixed');

    const refused = dispatch({ SetCapacity: { max_seats: 2 } }) as { Err: { message: string } };

    expect(refused.Err.message).toBe('cannot set max seats to 2; 3 seats are active');
    expect(dispatch('ShowCapacity')).toEqual({
      Ok: {
        max_seats: 3,
        memory_max_seats: 9,
        policy_max_seats: 12,
        cpu_max_seats: null,
        cpu_benchmark: {
          cpu_count: 16,
          iterations_per_sec: 12_600,
          single_core_iterations_per_sec: 810
        },
        disk_max_seats: null,
        monthly_data_budget: 'not_set',
        seat_monthly_data_gb: 65,
        data_max_seats: null,
        available_slots: 0
      }
    });
  });

  it('should raise the ceiling and report the freed slots', () => {
    resetState('seats-mixed');

    expect(dispatch({ SetCapacity: { max_seats: 5 } })).toEqual({
      Ok: {
        max_seats: 5,
        memory_max_seats: 9,
        policy_max_seats: 12,
        cpu_max_seats: null,
        cpu_benchmark: {
          cpu_count: 16,
          iterations_per_sec: 12_600,
          single_core_iterations_per_sec: 810
        },
        disk_max_seats: null,
        monthly_data_budget: 'not_set',
        seat_monthly_data_gb: 65,
        data_max_seats: null,
        available_slots: 2
      }
    });
    expect(dispatch('ShowCapacity')).toEqual({
      Ok: {
        max_seats: 5,
        memory_max_seats: 9,
        policy_max_seats: 12,
        cpu_max_seats: null,
        cpu_benchmark: {
          cpu_count: 16,
          iterations_per_sec: 12_600,
          single_core_iterations_per_sec: 810
        },
        disk_max_seats: null,
        monthly_data_budget: 'not_set',
        seat_monthly_data_gb: 65,
        data_max_seats: null,
        available_slots: 2
      }
    });
  });
});

describe('SetDataBudget', () => {
  // 179 GB over 65 GB a seat is 2.75: two whole seats, below the three running.
  it('should stop new sales below the running seats without refusing the budget', () => {
    resetState('seats-mixed');
    getState().maxSeats = 10;

    expect(dispatch({ SetDataBudget: { monthly_data_budget: '179' } })).toEqual({
      Ok: {
        max_seats: 10,
        memory_max_seats: 9,
        policy_max_seats: 12,
        cpu_max_seats: null,
        cpu_benchmark: {
          cpu_count: 16,
          iterations_per_sec: 12_600,
          single_core_iterations_per_sec: 810
        },
        disk_max_seats: null,
        monthly_data_budget: '179',
        seat_monthly_data_gb: 65,
        data_max_seats: 2,
        available_slots: 0
      }
    });
  });
});

describe('SetDataBudget answers', () => {
  it('should lift the data limit for a plan without a cap', () => {
    resetState('seats-mixed');
    getState().maxSeats = 10;
    dispatch({ SetDataBudget: { monthly_data_budget: '179' } });

    expect(dispatch({ SetDataBudget: { monthly_data_budget: 'unlimited' } })).toMatchObject({
      Ok: { monthly_data_budget: 'unlimited', data_max_seats: null, available_slots: 6 }
    });
  });

  it('should refuse to go back to no answer', () => {
    resetState('seats-mixed');
    dispatch({ SetDataBudget: { monthly_data_budget: 'unlimited' } });

    const refused = dispatch({ SetDataBudget: { monthly_data_budget: 'not_set' } }) as {
      Err: { message: string };
    };

    expect(refused.Err.message).toMatch(/monthly data cap/);
    expect(dispatch('ShowCapacity')).toMatchObject({ Ok: { monthly_data_budget: 'unlimited' } });
  });

  // The tokens MonthlyDataBudget::from_str refuses, so a dashboard bug that
  // sends one fails here as it would against the daemon.
  it.each([
    '0',
    '012',
    '1.5',
    '-1',
    '4294967296',
    ''
  ])('should refuse %j and keep the stored answer', (token) => {
    resetState('seats-mixed');
    dispatch({ SetDataBudget: { monthly_data_budget: '179' } });

    const refused = dispatch({
      SetDataBudget: { monthly_data_budget: token as MonthlyDataBudget }
    }) as { Err: { message: string } };

    expect(refused.Err.message).toMatch(/whole number of GB from 1/);
    expect(dispatch('ShowCapacity')).toMatchObject({ Ok: { monthly_data_budget: '179' } });
  });

  it('should accept the largest budget the daemon stores', () => {
    resetState('seats-mixed');

    expect(dispatch({ SetDataBudget: { monthly_data_budget: '4294967295' } })).toMatchObject({
      Ok: { monthly_data_budget: '4294967295' }
    });
  });
});

describe('guardian link', () => {
  it('should retain a terminal device while offering replacement and revoke both on cancel', () => {
    resetState('fedi-app-terminal');
    dispatch('CreateGuardianLinkOffer');

    expect(getState().guardianLink.link?.device_label).toBe('Pixel 8');
    expect(getState().guardianLink.offer?.uri).toContain('secret=mock-only');
    expect(dispatch('RevokeGuardianLink')).toEqual({
      Ok: { availability: 'ready', link: null, offer: null }
    });
  });

  it('should reject testing without a phone and deliver after loading a linked scenario', () => {
    resetState('fedi-app-unlinked');
    expect(dispatch('TestGuardianLinkNotification')).toEqual({
      Err: { kind: 'other', message: 'no phone linked' }
    });
    resetState('fedi-app-active');
    expect(dispatch('TestGuardianLinkNotification')).toEqual({
      Ok: { outcome: 'delivered', reason: null }
    });
    // The test notification carries no reason, so the told reasons stay.
    expect(getState().guardianLink.link?.notified_reasons).toEqual([
      'uptime_check_failed',
      'support_message'
    ]);
  });

  it('should not create an offer where the policy keeps mobile notifications off', () => {
    resetState('fedi-app-disabled');
    expect(dispatch('CreateGuardianLinkOffer')).toEqual({
      Err: { kind: 'other', message: 'mobile notifications are not enabled' }
    });
    expect(getState().guardianLink.offer).toBeNull();
  });

  it('should not create an offer without a push gateway', () => {
    resetState('fedi-app-unavailable');
    expect(dispatch('CreateGuardianLinkOffer')).toEqual({
      Err: { kind: 'other', message: 'no push gateway configured' }
    });
    expect(getState().guardianLink.offer).toBeNull();
  });
});

describe('ResetUptime', () => {
  it('should reopen an MG that only its uptime closed', () => {
    resetState('uptime-low');
    expect(dispatch('ShowSeatReadiness')).toMatchObject({ Ok: { ready_for_new_seats: false } });

    expect(dispatch('ResetUptime')).toEqual({ Ok: { uptime_reset: 'rechecking' } });

    expect(dispatch('ShowSeatReadiness')).toMatchObject({
      Ok: {
        ready_for_new_seats: true,
        report: {
          uptime: 'pass',
          uptime_comparison: { scores: { '1d': 950, '7d': 950, '30d': 950 } }
        }
      }
    });
  });

  it('should keep an MG closed while another check fails', () => {
    resetState('uptime-low');
    const report = getState().seatReadiness;
    if (!report) throw new Error('the scenario has a report');
    getState().seatReadiness = { ...report, bitcoin: 'bitcoin_syncing' };

    dispatch('ResetUptime');

    expect(dispatch('ShowSeatReadiness')).toMatchObject({
      Ok: { ready_for_new_seats: false, report: { uptime: 'pass', bitcoin: 'bitcoin_syncing' } }
    });
  });
});
