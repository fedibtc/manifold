import { routeToKey } from '@/mocks/routes';

it('should map the index route to the overview', () => {
  expect(routeToKey('/')).toBe('overview');
});

it('should map the seats list to seats', () => {
  expect(routeToKey('/seats')).toBe('seats');
});

it('should map a single seat to seat-detail rather than seats', () => {
  expect(routeToKey('/seats/seat-running-01')).toBe('seat-detail');
});

it('should map the wallet to wallet', () => {
  expect(routeToKey('/wallet')).toBe('wallet');
});

it('should map the offer to offer', () => {
  expect(routeToKey('/offer')).toBe('offer');
});

it('should map the backup index to backup', () => {
  expect(routeToKey('/backup')).toBe('backup');
});

it('should map the recovery phrase to backup-phrase rather than backup', () => {
  expect(routeToKey('/backup/phrase')).toBe('backup-phrase');
});

it('should map the authorization screen to authorization', () => {
  expect(routeToKey('/authorization')).toBe('authorization');
});

it('should tolerate a trailing slash', () => {
  expect(routeToKey('/seats/')).toBe('seats');
});

it('should return null for an unrouted path instead of claiming a screen', () => {
  expect(routeToKey('/nowhere')).toBe(null);
});
