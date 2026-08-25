import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

// The mock and the daemon both require a real 12-word BIP-39 phrase.
const PHRASE = `${'abandon '.repeat(11)}about`;

// The tracer bullet for setup: an operator arriving at a host that has never been
// onboarded, and leaving it selling seats. Every other fman spec enters through a
// scenario that is already onboarded.
const startSetup = async (page: import('@playwright/test').Page) => {
  await page.goto('/');
  await signIn(page);
  await expect(page.getByRole('heading', { name: 'Set up your fleet manager' })).toBeVisible();
};

test('should gate the whole app behind setup, with no sidebar', async ({ page }) => {
  await resetScenario(page, 'not-onboarded');

  await startSetup(page);

  await expect(page.getByRole('navigation', { name: 'Sections' })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Overview' })).toHaveCount(0);
});

test('should take a new fleet from the doors through to a priced offer', async ({ page }) => {
  await resetScenario(page, 'not-onboarded');

  await startSetup(page);
  await page.getByRole('button', { name: 'Start a new fleet' }).click();

  await expect(page.getByRole('heading', { name: 'Record your recovery phrase' })).toBeVisible();
  await page.getByRole('button', { name: 'Reveal phrase' }).click();
  await expect(page.getByText('abandon abandon abandon')).toBeVisible();
  await page.getByRole('button', { name: "I've written it down — continue" }).click();

  await expect(page.getByRole('heading', { name: 'Get this fleet authorized' })).toBeVisible();
  await page.getByRole('button', { name: 'Check now' }).click();
  await page.getByRole('button', { name: 'Continue now' }).click();

  await expect(page.getByRole('heading', { name: 'Set your price' })).toBeVisible();
  // The capacity field seeds from the daemon's RAM-derived recommendation; the
  // operator overrides it, and the override is what must land in the offer.
  await expect(page.getByLabel('Maximum active seats')).toHaveValue('8');
  await page.getByLabel('Maximum active seats').fill('5');
  await page.getByLabel('Price per seat (sats)').fill('50000');
  await page.getByRole('button', { name: 'Finish setup' }).click();

  // The final stage is durable but the fleet has not opened: the daemon
  // reports `runtime: starting` and the gate holds the wizard until `ready`.
  await expect(page.getByRole('heading', { name: 'Set your price' })).toBeVisible();

  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  await expect(page.getByText('50,000 sats per seat')).toBeVisible();

  // The chosen capacity is durable, not just echoed by the configure response.
  const capacity = await page.evaluate(async () => {
    const response = await fetch('/api/admin', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify('ShowCapacity')
    });
    return response.json();
  });
  expect(capacity).toEqual({ Ok: { max_seats: 5, available_slots: 5 } });
});

// The daemon's stage cursor does not advance past `holder_authorization` until
// an authorization is retained — there is no skip (SPEC-admin-socket). The
// wizard says so with a disabled Continue rather than an error after the fact.
test('should hold setup at the authorization step until one is observed', async ({ page }) => {
  await resetScenario(page, 'not-onboarded');

  await startSetup(page);
  await page.getByRole('button', { name: 'Start a new fleet' }).click();
  await page.getByRole('button', { name: 'Reveal phrase' }).click();
  await page.getByRole('button', { name: "I've written it down — continue" }).click();

  await expect(page.getByText(/No authorization for this fleet/i)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeDisabled();
});

test('should require the acknowledgement before recovering from a phrase', async ({ page }) => {
  await resetScenario(page, 'not-onboarded');

  await startSetup(page);
  await page.getByRole('button', { name: 'Recover from a phrase' }).click();

  await expect(page.getByRole('heading', { name: 'Recover from your phrase' })).toBeVisible();
  await page.getByLabel('Recovery phrase').fill(PHRASE);

  const recover = page.getByRole('button', { name: 'Recover this fleet' });
  await expect(recover).toBeDisabled();

  await page.getByLabel(/permanently offline/).check();
  await expect(recover).toBeEnabled();
});

// Recovery reports what the daemon did before it moves on, so the operator reads
// the counts and continues. A restored fleet then skips the QR when the relay
// already carries its authorization; this mock host is still waiting, which is
// the branch that stops here. Both branches are covered as units in SetupWizard's
// tests.
test('should recover a fleet and stop at the authorization step while it waits', async ({
  page
}) => {
  await resetScenario(page, 'not-onboarded');

  await startSetup(page);
  await page.getByRole('button', { name: 'Recover from a phrase' }).click();
  await page.getByLabel('Recovery phrase').fill(PHRASE);
  await page.getByLabel(/permanently offline/).check();
  await page.getByRole('button', { name: 'Recover this fleet' }).click();

  await expect(page.getByRole('heading', { name: 'Recovery finished' })).toBeVisible();

  await page.getByRole('button', { name: 'Continue' }).click();

  await expect(page.getByRole('heading', { name: 'Get this fleet authorized' })).toBeVisible();
});

test('should not offer setup to a fleet that is already running', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Set up your fleet manager' })).toHaveCount(0);
});
