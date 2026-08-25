# CLAIM-fi-client-bounded-discovery-complete: Bounded discovery retains every honest advertisement

An open-write relay publisher cannot make `FmanRegistryQuery` omit every honest,
eligible FMan advertisement when an honest advertisement occurs later in the
same enumeration.

## Status

Falsified: a valid 2,048-advertisement prefix can exclude a later honest,
eligible advertisement.

## Assumptions

- An open-write relay can enumerate distinct publisher advertisements before an
  honest publisher's advertisement.
- The publisher cannot forge the honest signature or compromise a pinned relay.
