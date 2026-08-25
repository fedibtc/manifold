import { describe, expect, it } from 'vitest';
import { panelConfig } from '@/mocks/panel-config';
import { getState, resetState } from '@/mocks/state';

const control = (id: string) => {
  const found = panelConfig.controls.find((entry) => entry.id === id);
  if (!found) throw new Error(`no control: ${id}`);
  return found;
};

describe('panelConfig setup controls', () => {
  it('should expose every stable recovery result', () => {
    expect(control('restoreResult').options).toEqual([
      '2 seats / 1 formed',
      '2 seats / 0 formed',
      '0 seats'
    ]);
  });

  it('should write the selected recovery result into the world', () => {
    resetState('not-onboarded');
    control('restoreResult').write('0 seats');

    expect(getState().restoreResult).toBe('no-seats');
  });

  it('should read the world back as its display label', () => {
    resetState('not-onboarded');
    getState().restoreTransport = 'fail-after-commit';

    expect(control('restoreTransport').read()).toBe('fail after commit');
  });

  it('should reset every setup control with the scenario', () => {
    resetState('not-onboarded');
    control('restoreTransport').write('fail before dispatch');
    control('restoreSession').write('expire on submit');

    resetState('fresh-fleet');

    expect(getState().restoreTransport).toBe('normal');
    expect(getState().restoreSession).toBe('active');
  });

  it('should offer the real classes of recovery refusal', () => {
    expect(panelConfig.errors.codes).toContain('invalid mnemonic');
    expect(panelConfig.errors.codes).toContain(
      'backup document version is newer than this build can read'
    );
  });
});
