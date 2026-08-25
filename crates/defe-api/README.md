# defe-api

Shared wire API types for the local `defe` test resource server and its clients.
The crate exposes CBOR frame helpers plus request and response data structures used
on the Unix socket named by `DEV_DEFE_SOCKET_PATH`.

## Protocol contract

Each message is a single frame: a four-byte big-endian payload length followed by
one CBOR value. The payload is encoded with Serde/ciborium. Request and response
enums use externally tagged PascalCase variant names, while struct fields use
snake_case names. `encode_frame` and `decode_frame` reject payloads larger than
`MAX_FRAME_SIZE` (1 MiB); `decode_frame` also rejects incomplete frames and any
trailing bytes after the declared payload.

This is a local, same-version protocol between the `defe` crate components. The
wire format is covered by golden tests to catch accidental changes, but it is not
a long-term compatibility promise across independently upgraded releases. Path
fields, including `NostrRelayInfo::data_dir` and
`PushGatewayInfo::database_path`, are expected to be valid UTF-8 for portable
CBOR exchange. `ResourceRequest` currently supports `NostrRelay` and
`PushGateway`; both use the same scoped handle, explicit release, restart, and
connection-drop cleanup rules.
