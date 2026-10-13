import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';

// readdirSync's own recursion, so the scan below needs no branch of its own —
// test code here carries no conditionals. Vitest runs with the app directory as
// its working directory.
const APP_SRC = join(process.cwd(), 'src');

const APP_SOURCES = readdirSync(APP_SRC, { recursive: true, encoding: 'utf8' }).filter(
  (relativePath) => /\.tsx?$/.test(relativePath)
);

const SELF = join('mocks', 'world', '__tests__', 'keys.test.ts');

// The ban is on npub-shaped *fixtures* — values standing in for something the
// daemon sent. These two files instead assert the npub the UI renders *from* a
// hex wire value, which is the one place the literal is the subject rather than
// a stand-in. Their own fixtures stay hex, so the rule below still binds them.
const RENDERING_CONTRACT_FILES = [
  join('shared', 'utils', '__tests__', 'npub.test.ts'),
  join('pages', 'authorization', '__tests__', 'AuthorizationPage.test.tsx')
];

// The daemon serialises nostr_sdk::PublicKey with to_string(), which returns 64
// lowercase hexadecimal characters — never an npub. A fixture in any other shape
// tests a wire value that cannot occur.
const HEX_64 = /^[0-9a-f]{64}$/;

describe('mock nostr keys', () => {
  it('should expose a service key in the daemon wire format', () => {
    expect(MOCK_SERVICE_NOSTR_PUBKEY).toMatch(HEX_64);
  });

  it('should expose a holder key in the daemon wire format', () => {
    expect(MOCK_HOLDER_PUBKEY).toMatch(HEX_64);
  });

  it('should not reuse one value for both roles', () => {
    expect(MOCK_SERVICE_NOSTR_PUBKEY).not.toBe(MOCK_HOLDER_PUBKEY);
  });

  // The two assertions above only prove the shape of the constants this file
  // owns. A new npub literal dropped into any other fixture would leave them
  // green, so the sweep itself is what needs guarding — otherwise the rule holds
  // only for as long as nobody adds a fixture.
  // A scan that found no files would make the check below pass for the wrong
  // reason, and a vacuous guard is worse than none.
  it('should read the whole app source', () => {
    expect(APP_SOURCES.length).toBeGreaterThan(100);
  });

  // An exemption that outlives the file it names would silently widen the ban's
  // blind spot, so the allowlist is checked against the tree it filters.
  it('should exempt only files that still exist', () => {
    expect(
      RENDERING_CONTRACT_FILES.filter(
        (exempt) => !APP_SOURCES.some((relativePath) => relativePath.endsWith(exempt))
      )
    ).toEqual([]);
  });

  it('should find no npub literal anywhere in the app source', () => {
    const exempt = [SELF, ...RENDERING_CONTRACT_FILES];
    const offenders = APP_SOURCES
      // This file names the forbidden prefix in order to look for it.
      .filter((relativePath) => !exempt.some((allowed) => relativePath.endsWith(allowed)))
      .filter((relativePath) =>
        readFileSync(join(APP_SRC, relativePath), 'utf8').includes('npub1')
      );

    expect(offenders).toEqual([]);
  });
});
