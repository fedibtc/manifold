# Current argument

## Argument

The current Admin request requires a nonempty client `withdrawal_intent_id`,
checks an existing binding before preparation and again inside the serialized
write transaction, and binds it through a unique partial index. Existing-intent
replay requires identical address, amount, and resolved fee and returns the
stored operation without submitting. The winning transaction commits `in_doubt`
before the one external send, so crash and replay do not create a second
invocation under A1-A3. Distinct intent ids continue to permit deliberately
identical withdrawals.

## Residuals

The Admin bearer identifies the installation, not stable individual operator
principals. This proof establishes installation-scoped intent uniqueness under
the claim's A1 boundary; a stronger per-human namespace needs a separate auth
property.

## Weakest links

The proof depends on the complete request/replay call-site enumeration and the
partial unique index remaining aligned with the service checks.
