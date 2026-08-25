import { RuleTester } from 'eslint';
import rule from '../no-inline-jsx-map.mjs';

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2022,
    sourceType: 'module',
    parserOptions: { ecmaFeatures: { jsx: true } },
  },
});

ruleTester.run('no-inline-jsx-map', rule, {
  valid: [
    {
      name: 'named render function',
      code: `
        const renderRow = (row) => <InvoiceRow key={row.id} {...row} />
        const InvoiceTable = ({ rows }) => <tbody>{rows.map(renderRow)}</tbody>
      `,
    },
    {
      name: 'map not returning JSX',
      code: `
        const InvoiceTotals = ({ rows }) => {
          const amounts = rows.map((row) => row.amount)
          return <span>{amounts.length}</span>
        }
      `,
    },
  ],
  invalid: [
    {
      name: 'inline map returning JSX inside JSX',
      code: `
        const InvoiceTable = ({ rows }) => (
          <tbody>{rows.map((row) => <InvoiceRow key={row.id} {...row} />)}</tbody>
        )
      `,
      errors: [{ messageId: 'inlineMap' }],
    },
  ],
});

console.log('no-inline-jsx-map: all tests passed');
