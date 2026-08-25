/**
 * Harness ESLint flat-config fragment. Spread into the target project's
 * eslint.config.mjs:
 *
 *   import harness from './eslint/base.config.mjs'
 *   export default [...yourExistingConfig, ...harness]
 *
 * Plugin deps (installer handles): eslint-plugin-react, eslint-plugin-jest,
 * eslint-plugin-playwright, eslint-plugin-sonarjs (optional).
 */
import boundaries from './boundaries.config.mjs';
import localHarness from './rules/index.mjs';

const BANNED_IDENTIFIERS = [
  'Wrapper', 'Container', 'Helper', 'Manager', 'Utils', 'data', 'temp', 'foo', 'obj',
];

const SRC = ['**/*.{ts,tsx,js,jsx}'];
const TESTS = ['**/*.{test,spec}.{ts,tsx,js,jsx}'];
const E2E = ['e2e/**/*.{ts,js}', '**/*.e2e.{ts,js}'];

export default [
  // Feature-based structure boundaries — §10 (no-op until plugin installed)
  ...boundaries,
  {
    files: SRC,
    plugins: { local: localHarness },
    rules: {
      // Naming — docs/clean-code.md §1
      'id-denylist': ['error', ...BANNED_IDENTIFIERS],
      'no-restricted-syntax': [
        'error',
        {
          selector: 'JSXAttribute[name.name=/^set[A-Z]/]',
          message:
            'Do not pass setState setters as props. Colocate the draft state; expose onChange/onCommit (docs/clean-code.md §2).',
        },
        {
          selector: 'JSXAttribute[name.name=/^(dispatch|setState)$/]',
          message:
            'Do not pass dispatch/setState as props. Colocate state; expose onChange/onCommit (docs/clean-code.md §2).',
        },
        {
          selector:
            'VariableDeclarator[id.name=/(Wrapper|Container|Component)\\d*$/][init.type=/ArrowFunctionExpression|FunctionExpression/]',
          message:
            'Structural component name — name it for domain purpose: PageHeader, CheckoutSummary (docs/clean-code.md §1).',
        },
        {
          selector: "Identifier[name=/^(handleClick|data|temp)\\d+$/]",
          message: 'Numeric-suffix identifier — pick a meaningful name (docs/clean-code.md §1).',
        },
        {
          selector: "TSTypeReference[typeName.name='Dispatch']",
          message:
            'Dispatch<SetStateAction<...>> in a type usually means a setter is crossing a component boundary — expose onChange/onCommit instead (docs/clean-code.md §2).',
        },
      ],
      // Style — §5
      'func-style': ['error', 'expression', { allowArrowFunctions: true }],
      'no-magic-numbers': [
        'warn',
        { ignore: [-1, 0, 1, 2], ignoreArrayIndexes: true, ignoreDefaultValues: true, enforceConst: true },
      ],
      // Proxy metrics — §3
      complexity: ['error', 10],
      'max-depth': ['error', 3],
      'max-params': ['error', 3],
      'max-lines': ['warn', { max: 300, skipBlankLines: true, skipComments: true }],
      'max-lines-per-function': ['warn', { max: 80, skipBlankLines: true, skipComments: true }],
      // Custom — §2, §5
      'local/no-courier-hooks': 'error',
      'local/max-usestate-per-component': ['error', { max: 4 }],
      'local/no-inline-jsx-map': 'error',
    },
  },
  {
    // Same clean-code standard applies to ALL tests — §7
    files: [...TESTS, ...E2E],
    plugins: { local: localHarness },
    rules: {
      'local/sut-import-last': 'error',
      'no-magic-numbers': 'off',
      'max-lines-per-function': 'off',
    },
  },
  /*
   * Installer appends (once plugins are installed):
   * - react: { 'react/jsx-handler-names': ['error', { eventHandlerPrefix: 'handle', eventHandlerPropPrefix: 'on' }] }
   * - jest (TESTS): valid-title should-prefix, no-conditional-in-test, no-conditional-expect
   * - playwright (E2E): no-wait-for-timeout, prefer-web-first-assertions, no-conditional-in-test
   * - sonarjs: cognitive-complexity 15
   */
];
