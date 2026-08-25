import { describe, expect, it } from 'vitest';
import { formatAge, formatRelative, parseTimestamp } from '../time';

describe('parseTimestamp', () => {
  it('should resolve a numeric unix-seconds timestamp', () => {
    expect(parseTimestamp(1721476800)).toBe(1721476800);
  });

  it('should return null for missing values', () => {
    expect(parseTimestamp(null)).toBeNull();
    expect(parseTimestamp(undefined)).toBeNull();
  });
});

describe('formatAge', () => {
  it('should render sub-5s as "just now"', () => {
    expect(formatAge(0)).toBe('just now');
    expect(formatAge(4)).toBe('just now');
  });

  it('should render seconds, minutes, hours and days', () => {
    expect(formatAge(42)).toBe('42s ago');
    expect(formatAge(5 * 60)).toBe('5m ago');
    expect(formatAge(3 * 3600)).toBe('3h ago');
    expect(formatAge(2 * 86400)).toBe('2d ago');
  });

  it('should clamp negatives to "just now"', () => {
    expect(formatAge(-10)).toBe('just now');
  });
});

describe('formatRelative', () => {
  it('should combine parse + age against now', () => {
    const now = 1721476800 + 300;
    expect(formatRelative(1721476800, now)).toBe('5m ago');
  });

  it('should return an em dash for missing timestamps', () => {
    expect(formatRelative(null, 1721476800)).toBe('—');
  });
});
