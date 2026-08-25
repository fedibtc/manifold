import { routeToKey } from '@/mocks/routes';

it('should map the index route to the overview', () => {
  expect(routeToKey('/')).toBe('overview');
});

it('should map the setup wizard to setup', () => {
  expect(routeToKey('/setup')).toBe('setup');
});

it('should map funds to funds', () => {
  expect(routeToKey('/funds')).toBe('funds');
});

it('should map allocations to allocations', () => {
  expect(routeToKey('/allocations')).toBe('allocations');
});

it('should map the advertisement to advertisement', () => {
  expect(routeToKey('/advertisement')).toBe('advertisement');
});

it('should map settings to settings', () => {
  expect(routeToKey('/settings')).toBe('settings');
});

it('should tolerate a trailing slash', () => {
  expect(routeToKey('/funds/')).toBe('funds');
});

it('should return null for an unrouted path instead of claiming a screen', () => {
  expect(routeToKey('/nowhere')).toBe(null);
});
