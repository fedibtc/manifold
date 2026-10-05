# SPEC-fman-support-chat: FMan operator chat with Fedi support

## Record justification

The chat is a wire contract between the FMan daemon and whatever NIP-17
client Fedi support uses, which this repository does not contain. Neither side
can document the other's obligations locally.

## Contract

An FMan operator and Fedi support exchange
[NIP-17](https://github.com/nostr-protocol/nips/blob/master/17.md) private
direct messages. There is one conversation per FMan and no tickets.

- **Identities.** The FMan writes as its service Nostr key, the key its
  advertisement and Holder authorization name. Fedi support is the one public
  key in the Manifold environment profile
  ([SPEC-manifold-environment](../../manifold-environment/specs/SPEC-manifold-environment.md)).
  A deployment without one has no chat: the daemon neither reads nor sends.
- **Messages.** Each message is a `kind:14` rumor with exactly one `p` tag,
  sealed (`kind:13`) by its author and gift-wrapped (`kind:1059`) per NIP-59.
  The FMan wraps every message twice: to Fedi, and to itself so that a
  reinstalled FMan can read its own side again.
- **Room.** The FMan admits a message only when the seal's signer is Fedi
  support and the rumor's only `p` tag is the FMan, or the signer is the FMan
  and the only `p` tag is Fedi support. Messages from other keys, rooms with
  more members, and other rumor kinds are ignored.
- **Relays.** The FMan publishes and reads on the environment's canonical
  relays and lists them in its `kind:10050` inbox relay list. Fedi support
  reads its own inbox on the same relays.
- **Identity of a message.** A message is identified by its rumor id, derived
  from the rumor rather than taken from it. Copies that relays serve again are
  stored once.
- **Length.** An operator message is 1 to 4000 characters after trimming.

The daemon stores the thread and a read mark in its database. Unread messages
are Fedi messages created after the mark; the mark only moves forward. Relays
are polled; a gift wrap backdates its timestamp up to two days, so each poll
reaches back that far.
