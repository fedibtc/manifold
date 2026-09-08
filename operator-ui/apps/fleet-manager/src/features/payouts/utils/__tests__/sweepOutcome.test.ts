import { describe, expect, it } from 'vitest';
import { describeCollection, describePayout } from '../sweepOutcome';

describe('describePayout', () => {
  it('should state what the sweep sent, in sats', () => {
    expect(
      describePayout({
        request_id: 'request-1',
        scope: { kind: 'payment_federation', federation_id: 'fed1aaa' },
        destination: 'operator@example.com',
        operation: { operation_id: 'op-1', amount_msat: 250_000_000, committed_at_ms: 2 },
        created_at_ms: 1
      })
    ).toBe('Sent 250,000 sats.');
  });
});

describe('describeCollection', () => {
  // A collection reports what it COULD take. Naming only the claimed amount
  // would read as "the account is empty now", which is false whenever a locked
  // deposit is waiting for the next cycle turnover.
  it('should state the locked remainder alongside what was claimed', () => {
    const sentence = describeCollection({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: '3000000'
    });

    expect(sentence).toContain('13,000 sats');
    expect(sentence).toContain('3,000 sats');
    expect(sentence).toBe(
      'Claimed 13,000 sats. 3,000 sats stay locked until the next cycle turnover.'
    );
  });

  it('should still state the waiting figure when nothing is locked', () => {
    const sentence = describeCollection({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: '0'
    });

    expect(sentence).toBe('Claimed 13,000 sats. 0 sats are waiting for the next cycle turnover.');
  });

  // Zero here is the daemon's answer, not a missing figure, so it is rendered as
  // a zero rather than as the unknown-amount dash.
  it('should render a claim of nothing as an explicit zero', () => {
    expect(
      describeCollection({
        claimed_msat: '0',
        recorded_claimed_msat: '13000000',
        awaiting_cycle_msat: '4000000'
      })
    ).toContain('Claimed 0 sats.');
  });

  // A collection that stopped partway counts only what a terminal operation
  // confirmed, so the claimed figure is a floor and the operator has to run it
  // again. Saying "Claimed X" flat would read as a finished job.
  it('should say the collection stopped and name the step it stopped at', () => {
    const sentence = describeCollection({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: '3000000',
      incomplete: { phase: 'unlock', operation_submitted: true, error: 'pool timed out' }
    });

    expect(sentence).toContain('at least 13,000 sats');
    expect(sentence).toContain('unlock step');
    expect(sentence).toContain('Run it again.');
  });

  // The post-failure balance read can itself fail. That is unknown, not zero:
  // rendering it as `0 sats are waiting` would invent a fact about the
  // operator's money, which is the one thing this module exists to prevent.
  it('should render an unreadable waiting balance as unknown, never as zero', () => {
    const sentence = describeCollection({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: null,
      incomplete: { phase: 'balance_refresh', operation_submitted: false, error: 'read failed' }
    });

    expect(sentence).toContain('—');
    expect(sentence).toContain('could not be read back');
    expect(sentence).not.toContain('0 sats are waiting');
  });

  it('should preserve max-u64 wire amounts while describing collection', () => {
    expect(
      describeCollection({
        claimed_msat: '18446744073709551615',
        recorded_claimed_msat: '18446744073709551615',
        awaiting_cycle_msat: '18446744073709551615'
      })
    ).toContain('18,446,744,073,709,551 sats');
  });
});
