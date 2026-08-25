import type { SeatStatusResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SeatRecoveryNotices } from '../SeatRecoveryNotices';

type Report = SeatStatusResponse['report'];

const render_ = (report: Report) => render(<SeatRecoveryNotices report={report} />);

describe('SeatRecoveryNotices', () => {
  it('should explain an unavailable seat as supervised and recovering', () => {
    render_({
      state: 'active',
      health: 'unavailable',
      phase: 'running',
      invite_code: 'fed11testinvite'
    });

    expect(screen.getByText(/supervised and currently recovering/i)).toBeTruthy();
  });

  it('should explain that a DKG-in-progress seat has no start/restart control', () => {
    render_({ state: 'active', health: 'healthy', phase: 'dkg_in_progress' });

    expect(screen.getByText(/no Start\/Restart control here/i)).toBeTruthy();
  });

  it('should distinguish durable guardian data loss from temporary recovery', () => {
    render_({
      state: 'active',
      health: 'unavailable',
      phase: 'data_loss',
      invite_code: 'fed11lostinvite'
    });

    expect(screen.getByText(/guardian data is missing/i)).toBeTruthy();
    expect(screen.queryByText(/not necessarily broken/i)).toBeNull();
  });

  it('should render nothing for a healthy running seat', () => {
    render_({ state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed1abc' });

    expect(screen.queryByText(/recovering/i)).toBeNull();
    expect(screen.queryByText(/Start\/Restart/i)).toBeNull();
  });
});
