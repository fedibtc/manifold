# CLAIM-manifold-guardian-payment-continuation-progresses: Accepted payments reach a terminal status

After a paid seat becomes durable, one recoverable payment hand-off error cannot
leave its payment permanently pending through a fair, fault-free suffix while the
daemon remains running.

## Status

Unverified.

## Assumptions

- The accepted payment is valid and the FA and payment federation behave
  honestly.
- After the one recoverable error, relevant I/O remains healthy and Tokio
  schedules every existing task.
- The daemon stays running, and the FA sends no new request.
