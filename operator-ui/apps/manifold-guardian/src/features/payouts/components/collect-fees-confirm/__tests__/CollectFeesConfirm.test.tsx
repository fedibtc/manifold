import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CollectFeesConfirm } from '../CollectFeesConfirm';

interface Options {
  collectableMsat?: number;
  isPending?: boolean;
}

const renderConfirm = ({ collectableMsat = 93_000, isPending = false }: Options = {}) => {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(
    <CollectFeesConfirm
      collectableMsat={collectableMsat}
      onConfirm={onConfirm}
      onCancel={onCancel}
      isPending={isPending}
    />
  );
  return { onConfirm, onCancel };
};

const panel = () => screen.getByRole('dialog');

describe('CollectFeesConfirm', () => {
  it('should name the amount the operator is about to collect', () => {
    renderConfirm();

    expect(panel()).toHaveAccessibleName('Collect 93 sats now?');
  });

  // The whole point of the panel. It must say a fee is taken without quoting a
  // total, because no fee estimate exists anywhere in the daemon or the UI.
  it('should warn that a mint fee is taken without promising a figure', () => {
    renderConfirm();

    expect(screen.getByText(/fee for every note/)).toBeInTheDocument();
    expect(screen.getByText(/approximately 0.1 sat each/)).toBeInTheDocument();
  });

  // "a little less" is true at a few hundred sats and badly wrong at twenty,
  // where roughly twenty notes at 0.1 sat each take about a tenth of the
  // collection. The unqualified claim is the one that holds at every amount.
  it('should not soften how much a collection loses', () => {
    renderConfirm({ collectableMsat: 20_000 });

    expect(screen.queryByText(/a little less/)).not.toBeInTheDocument();
    expect(screen.getByText(/receive less than the pool shows/)).toBeInTheDocument();
  });

  it('should confirm the collection when the operator accepts', () => {
    const { onConfirm } = renderConfirm();

    fireEvent.click(screen.getByRole('button', { name: 'Collect anyway' }));

    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it('should cancel the collection when the operator declines', () => {
    const { onCancel } = renderConfirm();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('should name itself as a modal dialog', () => {
    renderConfirm();

    expect(panel()).toHaveAttribute('aria-modal', 'true');
  });

  it('should take focus so the prompt announces itself', () => {
    renderConfirm();

    expect(screen.getByRole('button', { name: 'Collect anyway' })).toHaveFocus();
  });

  it('should cancel on Escape', () => {
    const { onCancel } = renderConfirm();

    fireEvent.keyDown(panel(), { key: 'Escape' });

    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  // Escape while the collection is in flight would close the panel over a
  // request that is still running, leaving the operator with no report of it.
  it('should ignore Escape while the collection is in flight', () => {
    const { onCancel } = renderConfirm({ isPending: true });

    fireEvent.keyDown(panel(), { key: 'Escape' });

    expect(onCancel).not.toHaveBeenCalled();
  });

  it('should hold the cancel action while the collection is in flight', () => {
    renderConfirm({ isPending: true });

    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
  });
});
