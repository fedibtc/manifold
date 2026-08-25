import { renderHook } from '@testing-library/react';
import { vi } from 'vitest';
import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { useShellSummary } from '../useShellSummary';

vi.mock('@/shared/api/hooks/use-setup-state/useSetupState');

const useSetupStateMock = vi.mocked(useSetupState);

const arrangeSetupState = (
  data: { status?: string; config?: { network?: string } } | undefined
) => {
  useSetupStateMock.mockReturnValue({ data } as ReturnType<typeof useSetupState>);
};

it('should report ready with the configured network once setup completes', () => {
  arrangeSetupState({ status: 'ready', config: { network: 'signet' } });

  const { result } = renderHook(() => useShellSummary());

  expect(result.current.ready).toBe(true);
  expect(result.current.network).toBe('signet');
});

it('should report not ready while setup is unfinished', () => {
  arrangeSetupState({ status: 'pending_validation' });

  const { result } = renderHook(() => useShellSummary());

  expect(result.current.ready).toBe(false);
  expect(result.current.network).toBeUndefined();
});

it('should report not ready while setup state has not loaded', () => {
  arrangeSetupState(undefined);

  const { result } = renderHook(() => useShellSummary());

  expect(result.current.ready).toBe(false);
  expect(result.current.network).toBeUndefined();
});
