/** The daemon stores the seat limit as a u32 (`offer_state.max_seats`), so this
 *  is the ceiling for every field that writes one. */
export const MAX_SEAT_LIMIT = 4_294_967_295;
