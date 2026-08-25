import { AdminApiError, NetworkError } from '@/shared/api/errors';
import { isNotOnboardedError } from '../setupState';

it('should recognise the daemon refusal that means this host is not onboarded', () => {
  const error = new AdminApiError(
    'this Fleet Manager has not been onboarded yet: run `admin onboard new` or `admin onboard restore`',
    'not_onboarded'
  );

  expect(isNotOnboardedError(error)).toBe(true);
});

it('should still recognise the refusal after the daemon rewords its sentence', () => {
  const reworded = new AdminApiError('set this host up before asking it anything', 'not_onboarded');

  expect(isNotOnboardedError(reworded)).toBe(true);
});

it('should not open setup for an undifferentiated refusal that merely reads like one', () => {
  const impostor = new AdminApiError('this Fleet Manager has not been onboarded yet', 'other');

  expect(isNotOnboardedError(impostor)).toBe(false);
});

it('should not mistake an unrelated admin error for a missing setup', () => {
  expect(isNotOnboardedError(new AdminApiError('unknown seat'))).toBe(false);
});

it('should not mistake a network failure for a missing setup', () => {
  expect(isNotOnboardedError(new NetworkError())).toBe(false);
});
