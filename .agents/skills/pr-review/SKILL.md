---
name: pr-review
description: Review a pull request when someone asks for a review in Slack or on GitHub. Sets what to check, the output format, ownership, and follow-up.
---

# PR review

Use this skill for every PR review request, in Slack or on GitHub.

## Before you start

- One review per request. If another agent already took the request (it has
  :eyes: or a reply), or another agent owns the thread or channel, do not review.
- React with :eyes: when you take it.
- Read the linked issue or request first. Review against the intent, then the code.

## What to check

1. Does it do what the request asks, and fix the root cause?
2. Correctness: edge cases, error paths, races, restarts.
3. Tests: does a test fail on the old code and pass on the new code?
4. Compatibility: wire formats, persisted state, mixed versions
   (`specs/GATE-production-compatibility.md`).
5. Public repo hygiene: no people, federation, or host names.

Report only real findings. Skip style unless it hides a bug.

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
