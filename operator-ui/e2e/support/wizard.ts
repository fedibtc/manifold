import { expect, type Page } from '@playwright/test';

// The token prompt (G2) appears on the first authenticated call (401), once the
// boot sequence's health + setup-state queries resolve. Every call site invokes
// this immediately after a fresh page.goto(), so the field always eventually
// appears — await its visibility (web-first, auto-retrying) rather than a
// one-shot isVisible() snapshot, which races the async boot check and silently
// skips authentication if it runs before the prompt renders.
export const authenticate = async (
  page: Page,
  token = process.env.FLIP_ADMIN_TOKEN ?? 'e2e-token'
): Promise<void> => {
  const tokenField = page.getByLabel('Admin token');
  await expect(tokenField).toBeVisible();
  await tokenField.fill(token);
  await page.getByRole('button', { name: 'Continue' }).click();
};

// Fill a valid config across the seven wizard steps, landing on Review.
// Uses accessible selectors only (labels/roles), never brittle CSS.
export const completeWizard = async (page: Page): Promise<void> => {
  // Step 1 — Network: leave the select at signet.
  await expect(page.getByRole('heading', { name: 'Setup — Network' })).toBeVisible();
  await page.getByRole('button', { name: 'Continue' }).click();

  // Step 2 — Gateway. The identity is read from the gateway, never typed: it is
  // frozen at first setup and decides which gateway an accepted allocation pays,
  // so a typo would be permanent. Continue stays blocked until it has been read.
  await page.getByLabel('Gateway name').fill('e2e-gateway');
  await page.getByLabel('Admin URL').fill('https://gateway.local:8175');
  await page.getByLabel('Admin credential').fill('super-secret-cred');
  await page.getByRole('button', { name: 'Connect to gateway' }).click();
  await expect(page.getByText(/^Connected to /)).toBeVisible();
  await page.getByRole('button', { name: 'Continue' }).click();

  // Step 3 — Chain observer: backend stays Esplora.
  await page.getByLabel('URL', { exact: true }).fill('https://mempool.space/signet/api');
  await page.getByRole('button', { name: 'Continue' }).click();

  // Step 4 — Relays & endpoint. The Iroh node id derives from the provider
  // identity, so the daemon owns the advertised address and the operator
  // cannot type one.
  await page.getByRole('button', { name: 'Add relay' }).click();
  await page.getByLabel('Relay 1', { exact: true }).fill('wss://relay.fedi.social');
  await expect(page.getByLabel('Advertised address')).toBeDisabled();
  await page.getByLabel('Republish interval (seconds)').fill('3600');
  await page.getByRole('button', { name: 'Continue' }).click();

  // Step 5 — Policy & capacity.
  await page.getByLabel('Gateway', { exact: true }).check();
  await page.getByLabel('Low-balance warning (SATS)').fill('2000000');
  await page.getByLabel('Critical threshold (SATS)').fill('500000');
  await page.getByRole('button', { name: 'Add attester' }).click();
  await page.getByLabel('Attester 1 pubkey').fill('npub1fcs0k2h5q7d9x');
  await page.getByRole('button', { name: 'Continue' }).click();

  // Step 6 — Trust: attestations optional; skippable for wizard completion.
  await expect(page.getByRole('heading', { name: 'Trust', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Install' })).toBeVisible();
  await page.getByRole('button', { name: 'Continue' }).click();

  // Step 7 — Review.
  await expect(page.getByRole('heading', { name: 'Configuration' })).toBeVisible();
};
