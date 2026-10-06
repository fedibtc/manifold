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
  advertisement and Holder authorization name. Fedi support is the
  `support_nostr_pubkey` of the admitted setup-payment policy
  ([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)),
  which the policy publisher's signature authenticates. When the policy names
  none, it is the key the environment profile pins
  ([SPEC-manifold-environment](../../manifold-environment/specs/SPEC-manifold-environment.md)).
  Without either there is no chat: the daemon neither reads nor sends. Fedi
  starts with the profile key and adds the policy field only to rotate, once
  enough consumers accept the field.
- **Rotation.** A newer policy with another key moves the chat to that key:
  the daemon sends to it and admits only its room, reading the relays again
  from the start. Stored messages of the earlier key stay in the thread, but
  a reinstalled FMan reads back only the current key's room. Until an FMan
  admits the newer policy it still sends to, and admits, the earlier key, so
  Fedi watches the earlier inbox until its FMans have moved.
- **Messages.** Each message is a `kind:14` rumor with exactly one `p` tag,
  sealed (`kind:13`) by its author and gift-wrapped (`kind:1059`) per NIP-59.
  The FMan wraps every message twice: to Fedi, and to itself so that a
  reinstalled FMan can read its own side again.
- **Room.** The FMan admits a message only when the seal's signer is Fedi
  support and the rumor's only `p` tag is the FMan, or the signer is the FMan
  and the only `p` tag is Fedi support. Messages from other keys, rooms with
  more members, and other rumor kinds are ignored.
- **Relays.** Each side sends to the other's NIP-17 inbox, its newest
  `kind:10050` relay list. The FMan reads its own inbox on the environment's
  canonical relays and lists them there. It finds Fedi support's list on the
  same relays and sends to at most the first five relays it names. Until
  Fedi support publishes a list, the FMan cannot send. Its own copy of each
  message goes to its own inbox.
- **Identity of a message.** A message is identified by its rumor id, derived
  from the rumor rather than taken from it. Copies that relays serve again are
  stored once.
- **Length.** An operator message is 1 to 4000 characters after trimming.

The daemon stores the thread and a read mark in its database. Unread messages
are Fedi messages created after the mark; the mark only moves forward. The
daemon holds a live relay subscription for new gift wraps, and every five
minutes fetches what it missed while a relay was away. A gift wrap backdates
its timestamp up to two days, so each catch-up fetch reaches back that far.
The dashboard reads the stored thread, not the relays.
