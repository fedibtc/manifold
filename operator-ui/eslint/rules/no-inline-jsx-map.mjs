/**
 * no-inline-jsx-map
 * Prefer `items.map(renderItem)` over inline mapping returning JSX
 * (docs/clean-code.md §5). Extract a named render function.
 */

const returnsJsx = (fn) => {
  if (fn.body.type === 'JSXElement' || fn.body.type === 'JSXFragment') return true;
  if (fn.body.type === 'BlockStatement') {
    return fn.body.body.some(
      (statement) =>
        statement.type === 'ReturnStatement' &&
        statement.argument &&
        (statement.argument.type === 'JSXElement' || statement.argument.type === 'JSXFragment')
    );
  }
  return false;
};

export default {
  meta: {
    type: 'suggestion',
    docs: { description: 'Prefer items.map(renderItem) over inline JSX mapping' },
    schema: [],
    messages: {
      inlineMap:
        'Inline .map() returning JSX — extract a named render function: `const renderItem = (item) => <... />` then `items.map(renderItem)` (docs/clean-code.md §5).',
    },
  },
  create(context) {
    return {
      CallExpression(node) {
        const isMap =
          node.callee.type === 'MemberExpression' &&
          node.callee.property.type === 'Identifier' &&
          node.callee.property.name === 'map';
        if (!isMap) return;

        const [callback] = node.arguments;
        const isInlineFunction =
          callback &&
          (callback.type === 'ArrowFunctionExpression' || callback.type === 'FunctionExpression');
        if (!isInlineFunction || !returnsJsx(callback)) return;

        let parent = node.parent;
        while (parent && parent.type !== 'Program') {
          if (parent.type === 'JSXExpressionContainer') {
            context.report({ node, messageId: 'inlineMap' });
            return;
          }
          if (
            parent.type === 'FunctionDeclaration' ||
            parent.type === 'FunctionExpression' ||
            parent.type === 'ArrowFunctionExpression'
          ) {
            return;
          }
          parent = parent.parent;
        }
      },
    };
  },
};
