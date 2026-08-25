use serde::{Deserialize, Serialize};

/// Push recipient id.
///
/// For HTTP management/registration endpoints this is the authenticated
/// signer’s canonical lowercase hex Nostr public key.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RecipientId(pub String);
