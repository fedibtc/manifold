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
- **Relays.** The FMan publishes and reads on the environment's canonical
  relays and lists them in its `kind:10050` inbox relay list. Fedi support
  reads its own inbox on the same relays.
- **Identity of a message.** A message is identified by its rumor id, derived
  from the rumor rather than taken from it. Copies that relays serve again are
  stored once.
- **Length.** An operator message is 1 to 4000 characters after trimming.

The daemon stores the thread in its database, with a read flag on each Fedi
message. The dashboard marks read exactly the unread Fedi messages it shows,
by rumor id. A message stored later stays unread, even when it shares the
second or sorts earlier, and a read message never becomes unread again. The
daemon holds one relay subscription per support key. It asks each relay for
the newest 500 gift wraps addressed to the FMan, then stays open for new
ones. When a relay reconnects, the subscription is sent again and the relay
replays its newest 500, so messages sent while it was away still arrive. The
dashboard reads the stored thread, not the relays.

**Limited recovery.** The relays are the only copy outside the database. A
new install, and a change of support key, read back the newest 500 gift wraps
addressed to the FMan, and older history is not recovered. Anyone can address
wraps to the FMan's public key, so junk wraps count toward the 500.

**Sending.** A send answers once a relay accepts the copy to Fedi. The copy
to the FMan itself goes out in the background, so a relay that never answers
delays the operator's request by one acknowledgement timeout, not two.
