---
name: change-descriptions
description: Write commit and PR descriptions that preserve the request, motivation, contrast with the previous state, benefits, and noteworthy outcomes without duplicating system documentation.
---

# Change descriptions

Use this skill when writing or revising commit and PR descriptions.

Write first for an engineer who is new to the project. Start by establishing the
relevant domain or project context and why the component or previous behavior
matters; do not assume readers already know its names, architecture, or
motivation. Then give a concrete previous-versus-new behavior example and
explain why the difference matters. Put implementation and validation details
later, after the reader understands the decision and its consequences.

## Commit descriptions

- Include a detailed summary of the prompt and instructions that motivated the
  change.
- Focus on the motivation, the contrast with the previous state, the benefits,
  and other noteworthy outcomes—not on describing the implementation itself.
- Put details of how the system works in the relevant Linked Specs, not in the
  commit description.

## PR descriptions

- Begin by orienting a reader new to the project: establish relevant context,
  show the concrete previous-versus-new behavior, and explain why it matters.
- Compose the description by combining the relevant sections from every
  included commit.
- Preserve the commits' motivation, before-and-after context, benefits, and
  noteworthy outcomes.
- Put details of how the system works in the relevant Linked Specs, not in the
  PR description.
