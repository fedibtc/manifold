import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';

// @live specs run only under E2E_TARGET=daemon against a real, defe-provisioned
// Fleet Manager (via `just test-e2e-ui-fman`). They never run in mock mode.
// There is no scenario to reset here — the real daemon is the source of truth,
// and a freshly provisioned one owns no seats.
//
// UNVERIFIED AGAINST LIVE: written from static reads of live-daemon.spec.ts,
// OfferPage.tsx, and useOfferForm.ts — no live fman-stack was reachable in
// this session. Re-run against a real fman-stack and fix up
// selectors/timing/assertions in the next live-stack session before trusting
// this test.
test('@live should set the offer price and read it back after reload', async ({ page }) => {
  await page.goto('/offer');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Your offer', level: 1 })).toBeVisible();

  // Own the data: this is the smallest authenticated write available (C1's
  // hardened offer form) — pick a value this test controls end to end.
  const price = page.getByLabel('Price per seat (sats)');
  await price.fill('77000');
  await page.getByRole('button', { name: 'Save' }).click();

  // A successful save navigates to the overview (see useOfferForm.ts), so
  // return to the offer page and reload before reading the value back — this
  // asserts on round-tripped daemon state, not a UI echo of the submitted
  // value.
  await expect(page).toHaveURL('/');
  await page.goto('/offer');
  await page.reload();

  await expect(page.getByLabel('Price per seat (sats)')).toHaveValue('77000');
});
