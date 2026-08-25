// Feature-based folder boundaries — hard gate for the architecture rules in CLAUDE.md.
// Requires eslint-plugin-boundaries v7+: npm i -D eslint eslint-plugin-boundaries
//
// Layer patterns come from agent-toolkit.json ("boundaries" key) next to this file;
// without one, single-repo defaults apply (src/shared, src/features/*, src/app).
//   {
//     "boundaries": {
//       "shared": ["packages/*", "apps/*/src/shared"],
//       "feature": ["apps/*/src/features/*"],
//       "composition": ["apps/*/src/pages/*"], // optional glue layer (routes/pages)
//       "app": ["apps/*/src/app"]
//     }
//   }
// Layer roles: shared imports shared only; feature imports shared + its own folder;
// composition (optional) imports shared + features + composition; app imports everything.
// Constraint: all "feature" patterns need the same wildcard depth (same-feature matching
// compares every captured segment — same depth keeps captures aligned).
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import tsParser from '@typescript-eslint/parser';
import boundaries from 'eslint-plugin-boundaries';

const DEFAULTS = {
  shared: ['src/shared'],
  feature: ['src/features/*'],
  app: ['src/app']
};

let userConfig = {};
try {
  userConfig =
    JSON.parse(readFileSync(new URL('./agent-toolkit.json', import.meta.url), 'utf8')).boundaries ??
    {};
} catch {}
const layers = { ...DEFAULTS, ...userConfig };
// composition is optional and user-configured only — never defaulted (no assumed `pages`).
const composition = layers.composition ?? [];
const hasComposition = composition.length > 0;

// One capture name per * segment; same-feature = every captured segment equal.
const captureNames = (pattern) =>
  pattern
    .split('/')
    .filter((seg) => seg.includes('*'))
    .map((_, i) => `c${i}`);
const featureCaptures = captureNames(layers.feature[0]);
const sameFeature = Object.fromEntries(
  featureCaptures.map((c) => [c, `{{ from.element.captured.${c} }}`])
);

// include = directory roots whose whole tree gets linted (files outside a layer then error);
// without it, scope narrows to the layer dirs themselves.
const exts = '{js,jsx,ts,tsx,mjs,cjs}';
const allPatterns = [...layers.shared, ...layers.feature, ...composition, ...layers.app];
const files = (layers.include ?? allPatterns).map((p) => `${p}/**/*.${exts}`);

// The layering rule governs runtime coupling — what the shipped bundle may reach
// and in which direction. A test ships nowhere, and a colocated test is part of
// the unit it covers, not a second consumer of it. Linting them as ordinary layer
// members forces a fixture the mock world and the tests share to be either
// duplicated per test file, which is the drift a single owner exists to prevent,
// or moved into production `shared/` so that test data ships. Neither buys any
// runtime safety.
//
// Every test in this repo lives in a `__tests__/` folder (the structure gate
// requires it), so this one pattern covers all of them. Playwright specs sit in
// `apps/*/e2e/`, outside every layer pattern, so they were never linted here.
//
// The cost is real and accepted: a cross-feature import inside a test is no
// longer reported. Production code remains fully covered.
const ignores = ['**/__tests__/**'];

// Every app whose tsconfig declares "references" needs a tsconfig.eslint.json
// shim, or the resolver treats its tsconfig as a solution file and drops the
// "@/*" paths. The failure is silent and total: an unresolved "@/features/x"
// looks like the scoped package "@/features", so it is classified `external`,
// no policy applies to it, and no-unknown-dependencies stays quiet too. Since
// this repo mandates "@/" for all app imports, one missing shim leaves a whole
// app unchecked while the run still exits 0. Fail loudly instead.
const missingEslintTsconfigs = readdirSync('apps', { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .filter((app) => existsSync(`apps/${app}/tsconfig.json`))
  .filter((app) => !existsSync(`apps/${app}/tsconfig.eslint.json`));

if (missingEslintTsconfigs.length > 0) {
  throw new Error(
    `Missing tsconfig.eslint.json for: ${missingEslintTsconfigs.join(', ')}. ` +
      'Without it the "@/*" alias does not resolve and boundaries silently checks nothing. ' +
      'Create each file containing { "extends": "./tsconfig.json" }.'
  );
}

export default [
  {
    files,
    ignores,
    plugins: { boundaries },
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: 'latest',
        sourceType: 'module',
        ecmaFeatures: { jsx: true }
      }
    },
    settings: {
      'boundaries/elements': [
        ...layers.shared.map((pattern) => ({ type: 'shared', pattern })),
        ...layers.feature.map((pattern) => ({
          type: 'feature',
          pattern,
          capture: captureNames(pattern)
        })),
        ...composition.map((pattern) => ({ type: 'composition', pattern })),
        ...layers.app.map((pattern) => ({ type: 'app', pattern }))
      ],
      'import/resolver': {
        typescript: {
          alwaysTryTypes: true,
          noWarnOnMultipleProjects: true,
          // Apps resolve via tsconfig.eslint.json (extends tsconfig.json): a tsconfig with
          // "references" is treated as a solution file by the resolver and its "paths"
          // aliases are IGNORED — extends inherits paths but drops references.
          project: ['tsconfig.base.json', 'apps/*/tsconfig.eslint.json', 'packages/*/tsconfig.json']
        }
      }
    },
    rules: {
      'boundaries/no-unknown-files': 'error',
      'boundaries/no-unknown-dependencies': 'error',
      'boundaries/dependencies': [
        'error',
        {
          default: 'disallow',
          message:
            '{{ from.element.type }} may not import {{ to.element.type }} (feature-folder architecture)',
          policies: [
            {
              from: { element: { types: 'shared' } },
              allow: { to: { element: { types: 'shared' } } }
            },
            {
              from: { element: { types: 'feature' } },
              allow: { to: { element: { types: 'shared' } } }
            },
            // A feature may import itself — never a sibling feature.
            ...(featureCaptures.length
              ? [
                  {
                    from: { element: { types: 'feature' } },
                    allow: { to: { element: { types: 'feature', captured: sameFeature } } }
                  }
                ]
              : []),
            // Composition wires domains together: imports shared, features, and composition.
            ...(hasComposition
              ? [
                  {
                    from: { element: { types: 'composition' } },
                    allow: {
                      to: { element: { types: { anyOf: ['shared', 'feature', 'composition'] } } }
                    }
                  }
                ]
              : []),
            {
              from: { element: { types: 'app' } },
              allow: {
                to: {
                  element: {
                    types: {
                      anyOf: hasComposition
                        ? ['app', 'shared', 'feature', 'composition']
                        : ['app', 'shared', 'feature']
                    }
                  }
                }
              }
            }
          ]
        }
      ]
    }
  }
];
