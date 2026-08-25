import {
  describeSeatHealth,
  describeSeatPhase,
  describeSeatReport,
  isSeatForming,
  isSeatReportFinal
} from '../seatStatus';

it('should describe healthy as an ok chip', () => {
  expect(describeSeatHealth('healthy')).toEqual({ label: 'Healthy', tone: 'ok' });
});

it('should describe unavailable as a warn chip, not a failure', () => {
  expect(describeSeatHealth('unavailable')).toEqual({ label: 'Unavailable', tone: 'warn' });
});

it('should describe failed as a bad chip', () => {
  expect(describeSeatHealth('failed')).toEqual({ label: 'Failed', tone: 'bad' });
});

it('should label every seat phase', () => {
  expect(describeSeatPhase('created')).toBe('Created');
  expect(describeSeatPhase('dkg_in_progress')).toBe('DKG in progress');
  expect(describeSeatPhase('data_loss')).toBe('Data loss');
  expect(describeSeatPhase('running')).toBe('Running');
});

it('should describe a decommissioned report as neutral regardless of prior health', () => {
  expect(describeSeatReport({ state: 'decommissioned', at_ms: 1 })).toEqual({
    label: 'Decommissioned',
    tone: 'neutral'
  });
});

it('should call a seat forming until its phase reaches running', () => {
  expect(isSeatForming({ state: 'active', health: 'healthy', phase: 'dkg_in_progress' })).toBe(
    true
  );
  expect(
    isSeatForming({ state: 'active', health: 'healthy', phase: 'running', invite_code: 'invite' })
  ).toBe(false);
  expect(
    isSeatForming({
      state: 'active',
      health: 'unavailable',
      phase: 'data_loss',
      invite_code: 'invite'
    })
  ).toBe(false);
  expect(isSeatForming({ state: 'decommissioned', at_ms: 1 })).toBe(false);
});

// A running seat is settled, not finished: its health is still being reported.
it('should call only a decommissioned report final', () => {
  expect(isSeatReportFinal({ state: 'decommissioned', at_ms: 1 })).toBe(true);
  expect(
    isSeatReportFinal({
      state: 'active',
      health: 'healthy',
      phase: 'running',
      invite_code: 'invite'
    })
  ).toBe(false);
});

it('should describe an active report by its health', () => {
  expect(
    describeSeatReport({
      state: 'active',
      health: 'unavailable',
      phase: 'running',
      invite_code: 'invite'
    })
  ).toEqual({ label: 'Unavailable', tone: 'warn' });
});
