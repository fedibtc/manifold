import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SetupRestoreSuccess } from '../SetupRestoreSuccess';

describe('SetupRestoreSuccess', () => {
  it('should state the exact counts the daemon returned', () => {
    render(<SetupRestoreSuccess result={{ seats: 2, formed: 1 }} onContinue={vi.fn()} />);

    expect(screen.getByText('2')).toBeTruthy();
    expect(screen.getByText(/seat records/i)).toBeTruthy();
    expect(screen.getByText('1')).toBeTruthy();
    expect(screen.getByText(/guardian configuration/i)).toBeTruthy();
  });

  it('should not claim a recovered seat record is running', () => {
    render(<SetupRestoreSuccess result={{ seats: 2, formed: 1 }} onContinue={vi.fn()} />);

    expect(screen.queryByText(/running/i)).toBeNull();
    expect(screen.queryByText(/\bactive\b/i)).toBeNull();
  });

  it('should call out a recovery that found no seat records', () => {
    render(<SetupRestoreSuccess result={{ seats: 0, formed: 0 }} onContinue={vi.fn()} />);

    expect(screen.getByText(/no seat records/i)).toBeTruthy();
    expect(screen.getByText(/another valid phrase/i)).toBeTruthy();
    expect(screen.getByText(/cannot repeat setup on this host/i)).toBeTruthy();
  });

  it('should offer continue and nothing else', () => {
    const onContinue = vi.fn();
    render(<SetupRestoreSuccess result={{ seats: 0, formed: 0 }} onContinue={onContinue} />);

    expect(screen.getAllByRole('button')).toHaveLength(1);

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onContinue).toHaveBeenCalled();
  });
});
