import { RuleTester } from 'eslint';
import rule from '../no-courier-hooks.mjs';

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2022,
    sourceType: 'module',
    parserOptions: { ecmaFeatures: { jsx: true } },
  },
});

ruleTester.run('no-courier-hooks', rule, {
  valid: [
    {
      name: 'parent reads the value in its own logic',
      code: `
        const CheckoutPage = () => {
          const { total } = useCart()
          if (total === 0) return null
          return <CheckoutSummary total={total} />
        }
      `,
    },
    {
      name: 'fan-out to two children',
      code: `
        const InvoicePage = () => {
          const { invoice } = useInvoice()
          return (
            <>
              <InvoiceHeader invoice={invoice} />
              <InvoiceRows invoice={invoice} />
            </>
          )
        }
      `,
    },
    {
      name: 'useContext is exempt',
      code: `
        const ArticlePage = () => {
          const theme = useContext(ThemeContext)
          return <ArticleHeader theme={theme} />
        }
      `,
    },
    {
      name: 'suppressed with hoisted comment',
      code: `
        const CheckoutPage = () => {
          // hoisted: waterfall
          const { cart } = useCart()
          return <CheckoutSummary cart={cart} />
        }
      `,
    },
    {
      name: 'forwarding to a host element is usage',
      code: `
        const NameField = () => {
          const [name, setName] = useState('')
          return <input value={name} onChange={(e) => setName(e.target.value)} />
        }
      `,
    },
  ],
  invalid: [
    {
      name: 'courier: whole hook result forwarded to one child',
      code: `
        const CheckoutPage = () => {
          const form = useCheckoutForm()
          return <CheckoutFields form={form} />
        }
      `,
      errors: [{ messageId: 'courier' }],
    },
    {
      name: 'courier: destructured results all forwarded to one child',
      code: `
        const InvoicePage = () => {
          const { rows, total } = useInvoice()
          return <InvoiceTable rows={rows} total={total} />
        }
      `,
      errors: [{ messageId: 'courier' }],
    },
    {
      name: 'courier: spread into one child',
      code: `
        const ArticlePage = () => {
          const article = useArticle()
          return <ArticleBody {...article} />
        }
      `,
      errors: [{ messageId: 'courier' }],
    },
  ],
});

console.log('no-courier-hooks: all tests passed');
