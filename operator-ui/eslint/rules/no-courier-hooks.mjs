/**
 * no-courier-hooks
 * The courier test (docs/clean-code.md §2): if a component calls a hook and the
 * ONLY thing it does with the result is forward it to exactly ONE child
 * component, the hook call belongs in that child.
 *
 * Exempt: useContext, useRef, router/params hooks, `// hoisted:` comments.
 * Forwarding to host elements (<input value={value} />) is NOT flagged — that
 * is the component using the value to render.
 */

const EXEMPT_HOOKS =
  /^(useContext|useRef|useImperativeHandle|useId|useParams|useRouter|useNavigate|useNavigation|useLocation|useSearchParams|usePathname|useRoute)$/;

const isHookCall = (node) =>
  node &&
  node.type === 'CallExpression' &&
  ((node.callee.type === 'Identifier' && /^use[A-Z0-9]/.test(node.callee.name)) ||
    (node.callee.type === 'MemberExpression' &&
      node.callee.property.type === 'Identifier' &&
      /^use[A-Z0-9]/.test(node.callee.property.name)));

const hookNameOf = (node) =>
  node.callee.type === 'Identifier' ? node.callee.name : node.callee.property.name;

const elementNameOf = (openingElement) => {
  const { name } = openingElement;
  if (name.type === 'JSXIdentifier') return name.name;
  if (name.type === 'JSXMemberExpression') {
    const parts = [];
    let current = name;
    while (current.type === 'JSXMemberExpression') {
      parts.unshift(current.property.name);
      current = current.object;
    }
    parts.unshift(current.name);
    return parts.join('.');
  }
  return null;
};

/** Walk up from an identifier; return the JSX element name if the reference sits
 * inside a JSX attribute (or spread attribute) of a component-cased element,
 * 'host' for lowercase elements, or null if used outside JSX attributes. */
const enclosingJsxTarget = (identifier) => {
  let node = identifier;
  while (node.parent) {
    const { parent } = node;
    if (parent.type === 'JSXAttribute' || parent.type === 'JSXSpreadAttribute') {
      const opening = parent.parent;
      if (opening && opening.type === 'JSXOpeningElement') {
        const name = elementNameOf(opening);
        if (!name) return null;
        return /^[a-z]/.test(name) ? 'host' : name;
      }
      return null;
    }
    if (
      parent.type === 'FunctionDeclaration' ||
      parent.type === 'FunctionExpression' ||
      parent.type === 'ArrowFunctionExpression' ||
      parent.type === 'Program'
    ) {
      return null;
    }
    node = parent;
  }
  return null;
};

const hasHoistedComment = (sourceCode, node) => {
  const comments = [
    ...(sourceCode.getCommentsBefore(node) || []),
    ...sourceCode.getAllComments().filter((c) => c.loc.start.line === node.loc.start.line),
  ];
  return comments.some((c) => /hoisted:/.test(c.value));
};

export default {
  meta: {
    type: 'suggestion',
    docs: {
      description:
        'Hook results forwarded to exactly one child component belong in that child (courier test)',
    },
    schema: [],
    messages: {
      courier:
        "'{{hook}}' result is only forwarded to <{{child}}> — call it inside <{{child}}> instead (courier test, docs/clean-code.md §2). Suppress with `// hoisted: <reason>` if this is a legitimate lift.",
    },
  },
  create(context) {
    const sourceCode = context.sourceCode ?? context.getSourceCode();
    return {
      VariableDeclarator(node) {
        if (!isHookCall(node.init)) return;
        const hook = hookNameOf(node.init);
        if (EXEMPT_HOOKS.test(hook)) return;

        const declaration = node.parent;
        if (hasHoistedComment(sourceCode, declaration)) return;

        const variables = sourceCode.getDeclaredVariables
          ? sourceCode.getDeclaredVariables(node)
          : context.getDeclaredVariables(node);

        const readReferences = variables
          .flatMap((variable) => variable.references)
          .filter((reference) => reference.isRead());

        if (readReferences.length === 0) return;

        const targets = new Set();
        for (const reference of readReferences) {
          const target = enclosingJsxTarget(reference.identifier);
          if (target === null) return; // used in the component's own logic → legitimate
          targets.add(target);
        }

        if (targets.size === 1) {
          const [child] = targets;
          if (child === 'host') return; // rendering into a DOM element is usage
          context.report({ node, messageId: 'courier', data: { hook, child } });
        }
      },
    };
  },
};
