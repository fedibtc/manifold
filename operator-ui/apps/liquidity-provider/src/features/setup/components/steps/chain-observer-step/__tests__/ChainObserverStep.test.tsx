import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';
import { ChainObserverStep } from '@/features/setup/components/steps/chain-observer-step/ChainObserverStep';
import { type ConfigDraft, initialDraft } from '@/features/setup/services/draft';

const Harness = () => {
  const [draft, setDraft] = useState<ConfigDraft>(initialDraft);
  const onChange = (patch: Partial<ConfigDraft>) => setDraft((d) => ({ ...d, ...patch }));
  return <ChainObserverStep draft={draft} onChange={onChange} errors={{}} />;
};

describe('ChainObserverStep', () => {
  it('should switch backend shape between esplora and bitcoind', () => {
    render(<Harness />);
    expect(screen.queryByLabelText('Username')).toBeNull();

    const select = screen.getByLabelText('Backend') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'bitcoind' } });
    expect(screen.getByLabelText('Username')).toBeTruthy();
    expect(screen.getByLabelText('Password')).toBeTruthy();

    fireEvent.change(select, { target: { value: 'esplora' } });
    expect(screen.queryByLabelText('Username')).toBeNull();
  });
});
