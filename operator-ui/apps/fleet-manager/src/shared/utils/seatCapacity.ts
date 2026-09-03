/** The durable seat ceiling is a `u32` on the wire (`offer_state.max_seats`),
 *  so this is the widest value the daemon can store, not a UI preference. */
const MAX_SEAT_CEILING = 4_294_967_295;

/** What the seats field shows for a stored ceiling. */
export const formatSeatsField = (maxSeats: number): string => String(maxSeats);

export type ParsedSeats = { ok: true; maxSeats: number } | { ok: false; error: string };

/**
 * The field's shape only. The ceiling's real floor — it may not drop below the
 * seats that are still active — is not checked here: the active count is not on
 * the wire (`available_slots` is port-bounded, not `max_seats - active`), so
 * re-deriving it would drift from `Db::set_max_seats`. That refusal comes back
 * from the daemon and is shown as it arrives.
 */
export const parseSeatsField = (input: string): ParsedSeats => {
  const trimmed = input.trim();
  if (trimmed === '') return { ok: false, error: 'Enter a maximum number of seats.' };

  const maxSeats = Number(trimmed);
  if (!Number.isFinite(maxSeats)) return { ok: false, error: 'Enter a whole number of seats.' };
  if (!Number.isInteger(maxSeats)) return { ok: false, error: 'Seats cannot be fractional.' };
  if (maxSeats < 0) return { ok: false, error: 'Seats cannot be negative.' };
  if (maxSeats > MAX_SEAT_CEILING) {
    return { ok: false, error: `The most seats you can offer is ${MAX_SEAT_CEILING}.` };
  }

  return { ok: true, maxSeats };
};

/** How the stored ceiling reads under the seats field, in the operator's words.
 *  Both figures come straight off `ShowCapacity`; neither is derived. */
export const describeCapacity = (maxSeats: number, availableSlots: number): string => {
  const ceiling = `Currently ${maxSeats.toLocaleString('en-US')}`;
  if (maxSeats === 0) return `${ceiling} — no seats are offered.`;
  if (availableSlots === 0) return `${ceiling}, with no free slots left.`;
  return `${ceiling}, with ${availableSlots.toLocaleString('en-US')} free.`;
};
