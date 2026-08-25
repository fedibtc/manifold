# defe-portalloc

`defe-portalloc` provides cooperative, cross-process allocation of contiguous
IPv4 loopback (`127.0.0.1`) TCP/UDP port ranges for `defe` tests.

Reservations are recorded in a JSON state file under the user cache directory
(or `DEV_DEFE_PORTALLOC_DATA_DIR`) and protected by an advisory filesystem lock.
Each reservation is a short startup coordination window, not a renewable lease:
callers should bind the returned ports promptly and rely on operating-system
socket bindings for long-lived resources after the reservation expires.
