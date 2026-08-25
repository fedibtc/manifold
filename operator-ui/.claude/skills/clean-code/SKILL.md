---
name: clean-code
description: Naming and clean-code conventions. Apply when creating or renaming components, functions, variables, or files, and when refactoring or extracting shared components.
---

Read and apply `docs/clean-code.md` (the single source of truth). The five most-violated rules:

**1. Courier hooks** (§2)
❌ `const form = useCheckoutForm(); return <CheckoutFields form={form} />`
✅ Call `useCheckoutForm()` inside `CheckoutFields`; parent receives results via `onCommit`.

**2. Setters as props** (§2)
❌ `<NameField setValue={setFirstName} />`
✅ `<NameField defaultValue={firstName} onChange={handleNameChange} />`

**3. Structural names** (§1)
❌ `Wrapper`, `Container`, `FormHelper`, `data2`
✅ `PageHeader`, `CheckoutSummary`, `buildInvoiceRows`

**4. Premature extraction** (§4)
❌ Shared component from 2 coincidentally-similar blocks.
✅ Rule of three; varying element becomes `children`; name the extraction for domain purpose.

**5. Logic in JSX** (§5)
❌ Inline calculations, ternaries, `.map(item => <Row .../>)` in JSX.
✅ Named constants above JSX; `items.map(renderItem)`; curried handlers.
