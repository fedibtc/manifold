import noCourierHooks from './no-courier-hooks.mjs';
import maxUsestatePerComponent from './max-usestate-per-component.mjs';
import sutImportLast from './sut-import-last.mjs';
import noInlineJsxMap from './no-inline-jsx-map.mjs';

export default {
  meta: { name: 'eslint-plugin-local-harness', version: '1.0.0' },
  rules: {
    'no-courier-hooks': noCourierHooks,
    'max-usestate-per-component': maxUsestatePerComponent,
    'sut-import-last': sutImportLast,
    'no-inline-jsx-map': noInlineJsxMap,
  },
};
