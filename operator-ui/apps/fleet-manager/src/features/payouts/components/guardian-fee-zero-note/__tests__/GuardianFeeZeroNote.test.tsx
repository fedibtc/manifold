import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { GuardianFeeZeroNote } from '../GuardianFeeZeroNote';

describe('GuardianFeeZeroNote', () => {
  it('should ask the question the zeros raise', () => {
    render(<GuardianFeeZeroNote />);

    expect(screen.getByText('Why is this 0?')).toBeInTheDocument();
  });

  it('should keep the answer folded away until the operator asks', () => {
    render(<GuardianFeeZeroNote />);

    expect(screen.getByRole('group')).not.toHaveAttribute('open');
  });

  it('should name accrual and batching as the reason, and say nothing is lost', () => {
    render(<GuardianFeeZeroNote />);

    expect(screen.getByText(/Fees build up in members' apps first/)).toBeInTheDocument();
    expect(screen.getByText(/Nothing is lost while it waits/)).toBeInTheDocument();
  });

  it('should quote no rate, amount or deadline', () => {
    render(<GuardianFeeZeroNote />);

    expect(screen.getByText(/Members pay a small fee/).textContent).not.toMatch(
      /\d|\b(few|several|minutes?|hours?|days?|weeks?|months?)\b/i
    );
  });

  it('should tie the payout to each share and to how much members send', () => {
    render(<GuardianFeeZeroNote />);

    const answer = screen.getByText(/Members pay a small fee/).textContent;
    expect(answer).toMatch(/pays your share when that share reaches/);
    expect(answer).toMatch(/depends on how much members send/);
    expect(answer).not.toMatch(/every recipient/);
  });
});
