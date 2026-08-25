// React Compiler lint safety net — the highest-value first step.
// Surfaces Rules-of-React violations the compiler would otherwise skip *silently*
// (a bailed-out component just isn't memoized — no error, no memo). Land this and
// go green in CI BEFORE enabling the compiler, so flipping it on has no surprises.
//
// Requires eslint-plugin-react-hooks v6+ (compiler rules folded into recommended-latest)
// and a parser that understands JSX/TSX — @typescript-eslint/parser covers JS + TS + JSX + TSX:
//   npm i -D eslint eslint-plugin-react-hooks @typescript-eslint/parser
//
// Standalone: rename to eslint.config.mjs.
// Existing flat config (incl. the feature-folders boundaries config): both export arrays —
// concatenate them, e.g. export default [...boundaries, ...compiler]. If your existing config
// already sets a parser, you can drop the languageOptions block below.

import tsParser from '@typescript-eslint/parser';
import reactHooks from 'eslint-plugin-react-hooks';

// recommended-latest ships plugins + rules but no `files`/parser; scope it so ESLint 10
// actually lints JSX/TSX. Preset form auto-picks up new compiler rules (purity, immutability,
// refs, set-state-in-render, ...) on upgrade.
export default [
  {
    files: ['**/*.{js,jsx,ts,tsx,mjs,cjs}'],
    languageOptions: {
      parser: tsParser,
      parserOptions: { ecmaFeatures: { jsx: true }, sourceType: 'module' }
    },
    ...reactHooks.configs.flat['recommended-latest']
  }
];
