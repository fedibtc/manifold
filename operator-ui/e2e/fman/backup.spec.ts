import { expect, test } from '@playwright/test';
import { signIn } from './support/auth';
import { resetScenario } from './support/mock';

test('should present the derived service keys with a phrase-reveal action', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/backup');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Backup', level: 1 })).toBeVisible();
  await expect(page.getByText('Service pubkey')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Reveal recovery phrase' })).toBeVisible();
});

test('should say the phrase is the whole backup and offer no restore action', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/backup');
  await signIn(page);

  await expect(page.getByText(/recovery phrase is the whole backup/)).toBeVisible();
  await expect(page.getByText(/Recovery happens only while setting up a host/)).toBeVisible();
  await expect(page.getByRole('button', { name: /restore/i })).toHaveCount(0);
});

test('should reveal the phrase only after the operator asks', async ({ page }) => {
  await resetScenario(page, 'seats-mixed');

  await page.goto('/backup/phrase');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Reveal recovery phrase' })).toBeVisible();
  await page.getByRole('button', { name: 'Reveal phrase' }).click();

  await expect(page.getByText('abandon abandon abandon')).toBeVisible();
  await expect(page.getByText(/twelve words are a complete backup/)).toBeVisible();
});

test('should not promise the phrase can only ever be seen once', async ({ page }) => {
  // ShowMnemonic is a deliberately repeatable recovery verb: the daemon answers with the
  // phrase on every call, so this route can be walked again. The page used to promise the
  // opposite on both its screens.
  await resetScenario(page, 'seats-mixed');

  await page.goto('/backup/phrase');
  await signIn(page);

  await expect(page.getByRole('heading', { name: 'Reveal recovery phrase' })).toBeVisible();
  await expect(page.getByText(/exactly once|never re-displayed/i)).toHaveCount(0);

  await page.getByRole('button', { name: 'Reveal phrase' }).click();

  await expect(page.getByText('abandon abandon abandon')).toBeVisible();
  await expect(page.getByText(/exactly once|never re-displayed/i)).toHaveCount(0);
});
