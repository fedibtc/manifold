import { daysToRecover, formatDuration } from '../uptimeRecovery';

describe('daysToRecover', () => {
  it('should be zero at or above the minimum', () => {
    expect(daysToRecover('7d', 900, 900)).toBe(0);
    expect(daysToRecover('7d', 950, 900)).toBe(0);
  });

  it('should follow the moving average of each window', () => {
    // 1 − s decays as e^(−t/τ); the gate passes at the minimum minus half a 0.1% step.
    expect(daysToRecover('1d', 500, 700)).toBeCloseTo(Math.log(0.5 / 0.3005), 6);
    expect(daysToRecover('7d', 832, 900)).toBeCloseTo(7 * Math.log(0.168 / 0.1005), 6);
    expect(daysToRecover('30d', 914, 930)).toBeCloseTo(30 * Math.log(0.086 / 0.0705), 6);
  });

  it('should need only a moment one step under the minimum', () => {
    expect(daysToRecover('7d', 899, 900) * 24).toBeLessThan(1);
  });
});

describe('formatDuration', () => {
  it('should use hours under a day and a half, then days', () => {
    expect(formatDuration(0.01)).toBe('less than an hour');
    expect(formatDuration(1 / 24)).toBe('about 1 hour');
    expect(formatDuration(1.2)).toBe('about 29 hours');
    expect(formatDuration(3.6)).toBe('about 4 days');
  });
});
