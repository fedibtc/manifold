import { expect, test } from '@playwright/test';
import { authenticate, completeWizard } from './support/wizard';

// @live specs run only under E2E_TARGET=daemon against a real, defe-provisioned
// FLIP daemon (via `just test-e2e-ui-flip`). They never run in mock mode. There
// is no mock scenario to reset here — the real daemon is the source of truth.
//
// UNVERIFIED AGAINST LIVE: written from static reads of live-daemon.spec.ts,
// support/wizard.ts, SettingsPage.tsx, ReviewStep.tsx, and
// PolicyCapacityStep.tsx — no live flip-stack was reachable in this session.
// A freshly provisioned daemon starts unconfigured (see live-daemon.spec.ts),
// so this test drives the setup wizard to completion first — that path is
// itself unverified against a real daemon. Re-run against a real flip-stack
// and fix up selectors/timing/assertions in the next live-stack session
// before trusting this test.
test('@live should round-trip a settings change through the real daemon after setup', async ({
  page
}) => {
  await page.goto('/');
  await authenticate(page);

  // Reach a published, settings-capable daemon: complete the wizard and apply
  // it, mirroring the manual flow an operator would follow.
  await completeWizard(page);
  await page.getByRole('button', { name: 'Apply & go live' }).click();
  await expect(page.getByText("You're live")).toBeVisible();
  await page.getByRole('button', { name: 'Go to overview' }).click();

  // Own the data: this test picks a value it controls end to end and asserts
  // on the round-tripped daemon state after a reload, not a UI echo.
  await page.goto('/settings');

  const warningField = page.getByLabel('Low-balance warning (SATS)');
  await expect(warningField).toBeVisible();
  await warningField.fill('3300000');
  await page.getByRole('button', { name: 'Save changes' }).click();

  // The in-memory admin token does not survive a reload (see tokenStore.ts),
  // so re-authenticate before reading the persisted value back.
  await page.reload();
  await authenticate(page);

  await expect(page.getByLabel('Low-balance warning (SATS)')).toHaveValue('3300000');
});

// A6 Step 2, the half that had to wait for B3's confirmation panel: publish →
// verify status → confirmed withdraw → verify hidden. Each assertion is on the
// state the daemon reports after a reload, not on the UI's own echo of the
// action it just took.
//
// UNVERIFIED AGAINST LIVE: same caveat as the spec above — written from static
// reads of AdvertisementPage.tsx, WithdrawConfirm.tsx and support/wizard.ts,
// with no live flip-stack reachable in this session. Re-run against a real
// flip-stack and fix up selectors and waits before trusting it.
test('@live should publish an advertisement and hide it again on confirmed withdrawal', async ({
  page
}) => {
  await page.goto('/');
  await authenticate(page);

  await completeWizard(page);
  await page.getByRole('button', { name: 'Apply & go live' }).click();
  await expect(page.getByText("You're live")).toBeVisible();
  await page.getByRole('button', { name: 'Go to overview' }).click();

  await page.goto('/advertisement');

  // Publish, then read the status back from a fresh load of daemon state.
  await page.getByRole('button', { name: 'Republish now' }).click();
  await page.reload();
  await authenticate(page);
  await expect(page.getByText('Published')).toBeVisible();

  // Withdraw is two-step by design (B3): the first click must only open the
  // confirmation, and the advertisement must still be published at that point.
  await page.getByRole('button', { name: 'Withdraw advertisement' }).click();
  const confirmPanel = page.getByRole('group', { name: 'Withdraw this advertisement?' });
  await expect(confirmPanel).toBeVisible();
  await expect(page.getByText('Published')).toBeVisible();

  await confirmPanel.getByRole('button', { name: 'Confirm withdrawal' }).click();

  await page.reload();
  await authenticate(page);
  await expect(page.getByText('Withdrawn')).toBeVisible();
});
