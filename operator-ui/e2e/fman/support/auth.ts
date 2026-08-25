import { expect, type Page } from '@playwright/test';

// FMan gates every route behind an Onboarding liveness call; with no session
// cookie that call 401s and BootGate shows the sign-in prompt. Fill it to reach
// the app shell. The mock seeds the password per scenario ('test-password'); in
// daemon mode the runner passes the real one defe generated.
export const signIn = async (
  page: Page,
  password = process.env.FMAN_ADMIN_PASSWORD ?? 'test-password'
): Promise<void> => {
  await expect(page.getByRole('heading', { name: 'Sign in', level: 1 })).toBeVisible();
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
};
