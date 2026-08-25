import { RuleTester } from 'eslint';
import rule from '../sut-import-last.mjs';

const ruleTester = new RuleTester({
  languageOptions: { ecmaVersion: 2022, sourceType: 'module' },
});

const asTestFile = (code) => ({ code, filename: '/src/checkout/CheckoutSummary.test.tsx' });

ruleTester.run('sut-import-last', rule, {
  valid: [
    {
      name: 'SUT last',
      ...asTestFile(`
        import { render } from '@testing-library/react'
        import { buildCart } from './mocks'
        import { CheckoutSummary } from './CheckoutSummary'
      `),
    },
    {
      name: 'helpers after SUT-looking path are fine when they match helper patterns',
      ...asTestFile(`
        import { render } from '@testing-library/react'
        import { CheckoutSummary } from './CheckoutSummary'
        import { buildCart } from './test-utils/builders'
      `),
    },
    {
      name: 'non-test file ignored',
      code: `
        import { CheckoutSummary } from './CheckoutSummary'
        import { formatPrice } from './formatPrice'
      `,
      filename: '/src/checkout/CheckoutPage.tsx',
    },
  ],
  invalid: [
    {
      name: 'third-party import after SUT',
      ...asTestFile(`
        import { CheckoutSummary } from './CheckoutSummary'
        import { render } from '@testing-library/react'
      `),
      errors: [{ messageId: 'sutNotLast' }],
    },
  ],
});

console.log('sut-import-last: all tests passed');
