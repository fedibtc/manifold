import { expect, test } from '@playwright/test';
import { authenticate } from './support/wizard';

// @live specs run only under E2E_TARGET=daemon against a real, defe-provisioned
// FLIP daemon (via `just test-e2e-ui-flip`). They never run in mock mode. There
// is no mock scenario to reset here — the real daemon is the source of truth.

test('@live should reach the real daemon, authenticate, and gate a fresh install to setup', async ({
  page
}) => {
  await page.goto('/');
  await authenticate(page);

  // A freshly provisioned daemon is unconfigured, so the real not_configured
  // setup state raises the full-screen wizard in place of the shell. Setup
  // owns no route, so the assertion is on what renders, not on the URL.
  // Reaching this proves the whole live chain: the Vite proxy hits the real
  // admin API, the bootstrap token is accepted, and the real backend state
  // drives the UI. Write/action round-trips build on this tracer.
  await expect(page.getByRole('heading', { name: 'Setup — Network' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Sections' })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Overview' })).toHaveCount(0);
});
