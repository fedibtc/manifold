import { readySetupConfigView } from '@operator-ui/mock-fixtures';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as providerConfigHooks from '@/features/settings/api/hooks/use-provider-config/useProviderConfig';
import * as configSaveHooks from '@/features/settings/hooks/use-config-save/useConfigSave';
import { useProviderConfigForm } from '../useProviderConfigForm';

type ProviderConfigResult = ReturnType<typeof providerConfigHooks.useProviderConfig>;
type ConfigSaveResult = ReturnType<typeof configSaveHooks.useConfigSave>;

const mockProviderConfig = (partial: Partial<ProviderConfigResult>): void => {
  vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
    partial as unknown as ProviderConfigResult
  );
};

const mockIdleConfigSave = (): void => {
  vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue({
    save: vi.fn(),
    phase: 'idle',
    validation: null,
    isSaving: false
  } as unknown as ConfigSaveResult);
};

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>
);

const renderForm = () => renderHook(() => useProviderConfigForm(), { wrapper });

describe('useProviderConfigForm', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should report loading while the provider config is fetching', () => {
    mockProviderConfig({ isLoading: true });
    mockIdleConfigSave();

    const { result } = renderForm();

    expect(result.current.status).toBe('pending');
    expect(result.current.disposition).toEqual({ kind: 'loading' });
  });

  it('should report a failure with the query error when the provider config fails', () => {
    const error = new Error('service down');
    mockProviderConfig({ isError: true, error });
    mockIdleConfigSave();

    const { result } = renderForm();

    expect(result.current.status).toBe('pending');
    expect(result.current.disposition).toEqual({ kind: 'failed', error });
  });

  // The case the old two-way branch could not express. React Query keeps `data`
  // through a failed refetch, and the screen used to call that "no data" and
  // blank itself — deleting a config it still held, on the screen the operator
  // opened to read it. Two renders, because that is how the state arises: a
  // load that succeeded, then a refetch that did not.
  it('should keep the seeded draft and report staleness when a refresh fails', () => {
    mockProviderConfig({ isSuccess: true, data: { config: readySetupConfigView } });
    mockIdleConfigSave();

    const { result, rerender } = renderForm();
    expect(result.current.disposition).toEqual({ kind: 'content' });

    mockProviderConfig({
      isError: true,
      error: new Error('refresh failed'),
      data: { config: readySetupConfigView },
      dataUpdatedAt: 1_700_000_000_000
    });
    rerender();

    expect(result.current.status).toBe('ready');
    expect(result.current.disposition).toEqual({
      kind: 'stale',
      updatedAtMs: 1_700_000_000_000
    });
  });
});
