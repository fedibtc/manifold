//! Dedicated post-transfer metadata vote. Generic field writes cannot mutate
//! either the guardian directory or its fee recipients.

use std::collections::BTreeMap;

use fedi_decentralized_domain::{MG_SEAT_BINDINGS_META_FIELD_KEY, MgSeatBindings};
use fedi_decentralized_service_manifold_guardian::{
    FormationSeatBinding, MetaConsensusBase, rebind_transferred_seat,
};
use fedimint_meta_common::DEFAULT_META_KEY;

use super::{
    Seat, SeatCommand, SeatLoop, SeatVerbError, derive_federation_seats, meta_module_id,
    own_peer_id,
};

impl Seat {
    pub(crate) async fn replace_seat_binding(
        &self,
        expected_base: MetaConsensusBase,
        binding: FormationSeatBinding,
    ) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| SeatCommand::ReplaceSeatBinding {
            expected_base,
            binding,
            reply,
        })
        .await
    }
}

impl SeatLoop {
    pub(super) async fn replace_seat_binding(
        &mut self,
        expected_base: MetaConsensusBase,
        binding: &FormationSeatBinding,
    ) -> Result<(), SeatVerbError> {
        let (client, config) = self.running_client_config().await?;
        let meta_id = meta_module_id(&config).ok_or(SeatVerbError::MetaValueInvalid)?;
        let current = client
            .meta_get_consensus(meta_id, DEFAULT_META_KEY)
            .await
            .map_err(SeatVerbError::child_needed)?;
        if let Some(consensus) = &current {
            self.ensure_meta_object_size(consensus.value.as_slice(), "replace_seat_binding")?;
        }
        let actual_base = MetaConsensusBase::from_consensus(
            current
                .as_ref()
                .map(|consensus| (consensus.revision, consensus.value.as_slice())),
        );
        if actual_base != expected_base {
            return Err(SeatVerbError::MetaConsensusChanged);
        }
        let current = current.ok_or(SeatVerbError::MetaValueInvalid)?;
        let mut fields: BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(current.value.as_slice())
                .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        let directory = fields
            .get(MG_SEAT_BINDINGS_META_FIELD_KEY)
            .and_then(serde_json::Value::as_str)
            .ok_or(SeatVerbError::MetaValueInvalid)?;
        let recipients = fields
            .get(crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY)
            .and_then(serde_json::Value::as_str)
            .ok_or(SeatVerbError::MetaValueInvalid)?;
        let federation = derive_federation_seats(&config)?;
        MgSeatBindings::parse_canonical(directory)
            .and_then(|directory| directory.verify_for_federation(&federation))
            .map_err(|_| SeatVerbError::MetaValueInvalid)?;

        let (directory, recipients) =
            rebind_transferred_seat(directory, recipients, &binding.attestation)
                .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        // The shared helper predicts bytes; it is not an authentication
        // boundary. Verify the destination signature, federation/config hash,
        // exact native peer identity and endpoint proof before admitting a vote.
        let verified = MgSeatBindings::parse_canonical(&directory)
            .and_then(|directory| directory.verify_for_federation(&federation))
            .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        self.verify_endpoint_proofs(&config, std::slice::from_ref(binding))?;
        let own_peer = own_peer_id(&client).await?;
        if !verified
            .iter()
            .any(|entry| entry.peer_id == own_peer && entry.mg_pubkey == self.own_mg_pubkey)
        {
            return Err(SeatVerbError::MetaValueInvalid);
        }
        fields.insert(
            MG_SEAT_BINDINGS_META_FIELD_KEY.to_owned(),
            serde_json::Value::String(directory),
        );
        fields.insert(
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY.to_owned(),
            serde_json::Value::String(recipients),
        );
        let encoded = serde_json_canonicalizer::to_vec(&fields).map_err(SeatVerbError::internal)?;
        self.ensure_meta_object_size(&encoded, "replace_seat_binding")?;
        // Pin before the fallible RPC, exactly like ordinary metadata writes.
        // A successful call is one submitted vote, never consensus confirmation.
        self.submit_admitted_meta_target(
            &client,
            meta_id,
            expected_base,
            encoded,
            "replace_seat_binding",
        )
        .await
    }
}
