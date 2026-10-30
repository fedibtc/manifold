---
name: pr-review
description: Review a pull request when someone asks for a review in Slack or on GitHub. Sets the output format, ownership, and follow-up. Use the multipart-review skills for what to check.
---

# PR review

Use this skill for every PR review request, in Slack or on GitHub.

## Before you start

- One review per request. If another agent already took the request (it has
  :eyes: or a reply), or another agent owns the thread or channel, do not review.
- React with :eyes: when you take it.
- Read the linked issue or request first. Review against the intent, then the code.
- For what to check, use `multipart-review` and its parts. Pick only the parts
  the change needs.

## Output

Post the review on GitHub. Post a short pointer in Slack only when the request
came from Slack.

Format:

```
VERDICT: PASS | BLOCKER | NEEDS CLARIFICATION

1. <one finding, one or two sentences, with file:line>
2. ...

Not checked: <what you did not run or read>
```

- Start with one verdict. BLOCKER means it must not merge as is.
  NEEDS CLARIFICATION means you cannot decide without an answer.
- Short numbered points only. No summary of the diff, no praise, no essay.
- Put the blocking points first. Mark non-blocking points with `(nit)`.
- Use the GitHub event that matches: APPROVE for PASS, REQUEST_CHANGES for
  BLOCKER, COMMENT for NEEDS CLARIFICATION.
- The Slack pointer is one line: the verdict, the main point, and the link.

## After you post

- Watch the PR until it merges or closes: poll comments and reviews every
  2 minutes, and notify yourself only on new ones.
- Answer replies on GitHub yourself, without waiting for a Slack ping.
- When the author pushes a fix, re-review only the change, in the same format.
