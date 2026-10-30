---
name: pr-review-format
description: Format the output of a PR review, on GitHub or in Slack. Use whenever you post a review.
---

# PR review format

```
VERDICT: PASS | BLOCKER | NEEDS CLARIFICATION

1. <one finding, one or two sentences, with file:line>
2. ...

Not checked: <what you did not run or read>
```

- Start with one verdict. BLOCKER means it must not merge as is.
  NEEDS CLARIFICATION means you cannot decide without an answer.
- Short numbered points only. Blocking points first. Mark the others `(nit)`.
- No summary of the diff, no praise, no essay.
- On GitHub, use the matching event: APPROVE for PASS, REQUEST_CHANGES for
  BLOCKER, COMMENT for NEEDS CLARIFICATION.
- In Slack, post one line: the verdict, the main point, and the link.
- A re-review after a fix uses the same format and covers only the change.
