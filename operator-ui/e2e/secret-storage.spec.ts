import { expect, test } from '@playwright/test';
import { resetScenario } from './support/mock';
import { authenticate } from './support/wizard';

const SECRET_TOKEN = 'super-secret-token';
const SECRET_CREDENTIAL = 'credential-xyz';

test('should never persist the token or credentials to browser storage', async ({ page }) => {
  await resetScenario(page, 'setup-fresh');

  await page.goto('/');
  await authenticate(page, SECRET_TOKEN);

  // Walk to step 2 and fill the admin credential.
  await expect(page.getByRole('heading', { name: 'Setup — Network' })).toBeVisible();
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByLabel('Gateway name').fill('e2e-gateway');
  await page.getByLabel('Admin URL').fill('https://gateway.local:8175');
  await page.getByLabel('Admin credential').fill(SECRET_CREDENTIAL);

  const storage = await page.evaluate(() => ({
    local: JSON.stringify(localStorage),
    session: JSON.stringify(sessionStorage)
  }));

  expect(storage.local).not.toContain(SECRET_TOKEN);
  expect(storage.local).not.toContain(SECRET_CREDENTIAL);
  expect(storage.session).not.toContain(SECRET_TOKEN);
  expect(storage.session).not.toContain(SECRET_CREDENTIAL);

  // Defensive: no secret-looking key holds either value.
  const suspicious = await page.evaluate(() => {
    const scan = (store: Storage): string[] => {
      const hits: string[] = [];
      for (let i = 0; i < store.length; i += 1) {
        const key = store.key(i);
        if (key && /token|credential|secret/i.test(key)) {
          hits.push(store.getItem(key) ?? '');
        }
      }
      return hits;
    };
    return [...scan(localStorage), ...scan(sessionStorage)];
  });

  expect(suspicious).not.toContain(SECRET_TOKEN);
  expect(suspicious).not.toContain(SECRET_CREDENTIAL);
});
