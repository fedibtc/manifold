import { dispatch, isServiceErrorLike } from '@/mocks/world/verbs';

it('should throw an unavailable ServiceErrorLike with the mock-unavailable message for an unknown method', () => {
  expect.assertions(2);

  try {
    dispatch('not_a_real_method', null);
  } catch (error) {
    expect(isServiceErrorLike(error)).toBe(true);
    expect(error).toEqual({ code: 'unavailable', message: 'route not available in mock' });
  }
});
