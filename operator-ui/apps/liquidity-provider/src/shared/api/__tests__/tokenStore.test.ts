import { clearToken, getToken, setToken } from '../tokenStore';

it('should return null before any token is set', () => {
  clearToken();
  expect(getToken()).toBeNull();
});

it('should return the token after setToken', () => {
  setToken('abc123');
  expect(getToken()).toBe('abc123');
});

it('should clear the token with clearToken', () => {
  setToken('abc123');
  clearToken();
  expect(getToken()).toBeNull();
});
