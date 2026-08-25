-- Installation identifiers are only unique within a recipient.  A globally
-- unique installation_id lets any authenticated recipient who guesses or learns
-- the id delete/move another recipient's registration.  FCM token uniqueness
-- remains global because the token is the provider-issued device capability
-- used to move an actual device between recipients.
DROP INDEX IF EXISTS push_registrations_installation_id_idx;
