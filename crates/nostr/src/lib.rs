//! Nostr protocol constants and event admission shared by decentralized federation components.

pub mod attester;
pub mod flip;
pub mod fman;
pub mod setup_payment_federations;

/// Whether `event` carries exactly one `d` tag whose one value is `expected`.
///
/// Addressable-event admission must pin the exact identifier: a missing `d`
/// tag, a second `d` tag, or extra tag values must all fail to match rather
/// than admit an event under an ambiguous identity.
#[must_use]
pub fn has_exact_d_tag(event: &nostr::Event, expected: &str) -> bool {
    let mut d_tags = event
        .tags
        .as_slice()
        .iter()
        .filter(|tag| tag.kind() == nostr::TagKind::d());
    let Some(d_tag) = d_tags.next() else {
        return false;
    };
    let d_tag = d_tag.as_slice();
    d_tag.len() == 2 && d_tag[0] == "d" && d_tag[1] == expected && d_tags.next().is_none()
}
