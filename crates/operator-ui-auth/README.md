# Fedimint browser authentication helpers

This crate contains the browser-session authentication mechanism adapted from
`fedimint-ui-common` at the repository-pinned Fedimint revision. It
intentionally contains no Fedimint HTML, CSS, JavaScript, fonts, or images.

The local adaptation supports an explicit trusted-proxy mode and provides Tower
middleware so an entire API router can be protected at once. Keep relevant
authentication security fixes synchronized with upstream Fedimint.
