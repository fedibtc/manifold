import { formatWait } from '../formatWait';

describe('formatWait', () => {
  it('should use hours under a day and a half, then days', () => {
    expect(formatWait(600)).toBe('less than an hour');
    expect(formatWait(3600)).toBe('about 1 hour');
    expect(formatWait(29 * 3600)).toBe('about 29 hours');
    expect(formatWait(36 * 3600)).toBe('about 2 days');
    expect(formatWait(3.6 * 86400)).toBe('about 4 days');
  });
});
