// Loads a `u`-prefixed utilities CSS file into Tailwind's build config so the
// classes are registered as real components — always known to `@apply`, in
// every file, dev and build alike.
//
// Why this exists: a `.module.css` is processed by Vite/PostCSS *in isolation*,
// with no `@tailwind` context and no sight of the shared `utilities.css`. So
// `@apply uPageHeading` from a module fails in dev (Tailwind never saw the
// class), even though it works in a production build (which processes the whole
// CSS graph together). Tailwind's own docs prescribe the fix: define custom
// classes through the plugin system instead of `@apply`-ing them across
// isolated files. This keeps the CSS file as the editable dictionary and feeds
// it to the config, so the words are registered once and available everywhere.
//
// Usage (in a preset or app tailwind.config.cjs):
//   const plugin = require('tailwindcss/plugin');
//   const { utilitiesPlugin } = require('@operator-ui/common-ui/load-utilities');
//   module.exports = { plugins: [utilitiesPlugin(require.resolve('./styles/utilities.css'))] };

const { readFileSync } = require('node:fs');
const postcss = require('postcss');
const plugin = require('tailwindcss/plugin');

// Parse `.uName { @apply … ; <raw decls> }` rules (including any nested inside a
// `@layer` wrapper) into a Tailwind `addComponents` object. `@apply` becomes an
// at-rule key so Tailwind expands it; raw declarations pass through unchanged.
const componentsFromCss = (cssPath) => {
  const root = postcss.parse(readFileSync(cssPath, 'utf8'));
  const components = {};
  root.walkRules((rule) => {
    if (!rule.selector.startsWith('.')) return;
    const declaration = {};
    rule.walkAtRules('apply', (atRule) => {
      declaration[`@apply ${atRule.params}`] = {};
    });
    rule.walkDecls((decl) => {
      declaration[decl.prop] = decl.value;
    });
    if (Object.keys(declaration).length > 0) components[rule.selector] = declaration;
  });
  return components;
};

const utilitiesPlugin = (cssPath) =>
  plugin(({ addComponents }) => {
    addComponents(componentsFromCss(cssPath));
  });

module.exports = { utilitiesPlugin, componentsFromCss };
