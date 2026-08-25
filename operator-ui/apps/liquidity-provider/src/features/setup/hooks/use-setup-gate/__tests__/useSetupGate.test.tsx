import { renderHook } from '@testing-library/react';
import { vi } from 'vitest';
import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { useSetupGate } from '../useSetupGate';

vi.mock('@/shared/api/hooks/use-setup-state/useSetupState');

const useSetupStateMock = vi.mocked(useSetupState);

const arrangeSetupState = (data: { status: string } | undefined) => {
  useSetupStateMock.mockReturnValue({ data } as ReturnType<typeof useSetupState>);
};

it.each([
  ['not_configured'],
  ['pending_validation']
])('should gate while setup status is %s', (status) => {
  arrangeSetupState({ status });

  const { result } = renderHook(() => useSetupGate());

  expect(result.current.gated).toBe(true);
});

it('should not gate once setup status is ready', () => {
  arrangeSetupState({ status: 'ready' });

  const { result } = renderHook(() => useSetupGate());

  expect(result.current.gated).toBe(false);
});

it('should not gate while setup state has not loaded yet', () => {
  arrangeSetupState(undefined);

  const { result } = renderHook(() => useSetupGate());

  expect(result.current.gated).toBe(false);
});
