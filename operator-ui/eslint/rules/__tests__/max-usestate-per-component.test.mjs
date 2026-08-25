import { RuleTester } from 'eslint';
import rule from '../max-usestate-per-component.mjs';

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2022,
    sourceType: 'module',
    parserOptions: { ecmaFeatures: { jsx: true } },
  },
});

ruleTester.run('max-usestate-per-component', rule, {
  valid: [
    {
      name: 'under the cap',
      code: `
        const CheckoutForm = () => {
          const [name, setName] = useState('')
          const [email, setEmail] = useState('')
          return <form />
        }
      `,
    },
    {
      name: 'nested component counts separately',
      code: `
        const Outer = () => {
          const [a, setA] = useState(0)
          const [b, setB] = useState(0)
          const inner = () => {
            const [c, setC] = useState(0)
            const [d, setD] = useState(0)
            return null
          }
          return <div />
        }
      `,
      options: [{ max: 3 }],
    },
  ],
  invalid: [
    {
      name: 'over the cap',
      code: `
        const CheckoutForm = () => {
          const [a, setA] = useState('')
          const [b, setB] = useState('')
          const [c, setC] = useState('')
          const [d, setD] = useState('')
          const [e, setE] = useState('')
          return <form />
        }
      `,
      errors: [{ messageId: 'tooMany' }],
    },
    {
      name: 'custom max',
      code: `
        const Filters = () => {
          const [a, setA] = useState('')
          const [b, setB] = useState('')
          return <div />
        }
      `,
      options: [{ max: 1 }],
      errors: [{ messageId: 'tooMany' }],
    },
  ],
});

console.log('max-usestate-per-component: all tests passed');
