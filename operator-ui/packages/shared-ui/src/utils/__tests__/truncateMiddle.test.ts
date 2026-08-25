import { describe, expect, it } from 'vitest';
import { isTruncated, truncateMiddle } from '../truncateMiddle';

describe('truncateMiddle', () => {
  it('should leave a short value unchanged', () => {
    expect(truncateMiddle('short')).toBe('short');
  });

  it('should middle-ellipsis a long value using the default head and tail', () => {
    expect(truncateMiddle('abcdefghijklmnopqrstuvwxyz')).toBe('abcdefgh…wxyz');
  });

  it('should honor a custom head and tail', () => {
    expect(truncateMiddle('abcdefghijklmnop', 4, 4)).toBe('abcd…mnop');
  });

  it('should leave a value exactly at the threshold unchanged', () => {
    expect(truncateMiddle('123456789012345', 8, 6)).toBe('123456789012345');
  });
});

describe('isTruncated', () => {
  it('should be false for a short value', () => {
    expect(isTruncated('short')).toBe(false);
  });

  it('should be true for a value truncateMiddle would shorten', () => {
    expect(isTruncated('abcdefghijklmnopqrstuvwxyz')).toBe(true);
  });

  it('should be false for a value exactly at the threshold', () => {
    expect(isTruncated('123456789012345', 8, 6)).toBe(false);
  });
});
