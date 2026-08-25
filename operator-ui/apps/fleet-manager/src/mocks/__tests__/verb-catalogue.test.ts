// The mock is only evidence if it answers the same vocabulary the daemon does.
// A mock built from a second, hand-kept copy of the verb list is what kept the
// suite green while the wallet journey called a verb the daemon had deleted, so
// the catalogue is checked against the Rust-generated request inventory
// (crates/fman/core/tests/support/contract_fixtures.rs) rather than against
// another list in this repo.
import adminRequests from '@operator-ui/types/fixtures/fman_admin_requests.json';
import { describe, expect, it } from 'vitest';
import { adminMethods, MUTATING_VERBS, verbs } from '@/mocks/world/verbs';

const daemonVerbs = Object.keys(adminRequests);

describe('the mock verb catalogue against the daemon request inventory', () => {
  it('should answer every AdminRequest variant the daemon declares', () => {
    expect(daemonVerbs.filter((verb) => !(verb in verbs))).toEqual([]);
  });

  it('should answer nothing beyond that inventory', () => {
    expect(adminMethods.filter((verb) => !daemonVerbs.includes(verb))).toEqual([]);
  });

  it('should route every mutating verb it names', () => {
    expect([...MUTATING_VERBS].filter((verb) => !(verb in verbs))).toEqual([]);
  });
});
