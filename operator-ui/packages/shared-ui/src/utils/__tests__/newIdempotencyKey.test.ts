import { describe, expect, it } from 'vitest';
import { newIdempotencyKey } from '../newIdempotencyKey';

const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

describe('newIdempotencyKey', () => {
  it('should return a v4 UUID', () => {
    expect(newIdempotencyKey()).toMatch(UUID_V4);
  });

  it('should return a different key on each call', () => {
    const keys = new Set(Array.from({ length: 100 }, newIdempotencyKey));
    expect(keys.size).toBe(100);
  });

  // The bug this exists for: over plain http the dashboard is not a secure
  // context, `crypto.randomUUID` is undefined, and reading it during render
  // took down the whole Payouts screen.
  it('should still produce a key where crypto.randomUUID is undefined', () => {
    const original = Object.getOwnPropertyDescriptor(crypto, 'randomUUID');
    Object.defineProperty(crypto, 'randomUUID', { value: undefined, configurable: true });

    try {
      expect(newIdempotencyKey()).toMatch(UUID_V4);
    } finally {
      if (original) {
        Object.defineProperty(crypto, 'randomUUID', original);
      } else {
        Reflect.deleteProperty(crypto, 'randomUUID');
      }
    }
  });
});
