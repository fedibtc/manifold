import { describeCapacity, formatSeatsField, parseSeatsField } from '../seatCapacity';

it('should render a stored ceiling as the field text', () => {
  expect(formatSeatsField(4)).toBe('4');
  expect(formatSeatsField(0)).toBe('0');
});

it('should accept a whole number of seats', () => {
  expect(parseSeatsField('6')).toEqual({ ok: true, maxSeats: 6 });
});

it('should accept zero, which offers no seats at all', () => {
  expect(parseSeatsField('0')).toEqual({ ok: true, maxSeats: 0 });
});

it('should ignore surrounding whitespace', () => {
  expect(parseSeatsField('  3  ')).toEqual({ ok: true, maxSeats: 3 });
});

// The wire has no blank ceiling: max_seats is a u32, not nullable, so a cleared
// field is an unanswered question rather than "no limit".
it('should reject a blank field', () => {
  expect(parseSeatsField('')).toEqual({
    ok: false,
    error: 'Enter a maximum number of seats.'
  });
});

it('should reject a value that is not a number', () => {
  expect(parseSeatsField('lots')).toEqual({
    ok: false,
    error: 'Enter a whole number of seats.'
  });
});

it('should reject a fractional number of seats', () => {
  expect(parseSeatsField('2.5')).toEqual({ ok: false, error: 'Seats cannot be fractional.' });
});

it('should reject a negative ceiling', () => {
  expect(parseSeatsField('-1')).toEqual({ ok: false, error: 'Seats cannot be negative.' });
});

// The daemon stores the ceiling as a u32, so a larger number would not survive
// the write it appears to have made.
it('should reject a ceiling past what the daemon can store', () => {
  expect(parseSeatsField('4294967296')).toEqual({
    ok: false,
    error: 'The most seats you can offer is 4294967295.'
  });
});

it('should accept the largest ceiling the daemon can store', () => {
  expect(parseSeatsField('4294967295')).toEqual({ ok: true, maxSeats: 4_294_967_295 });
});

it('should describe the stored ceiling and its free slots', () => {
  expect(describeCapacity(4, 2)).toBe('Currently 4, with 2 free.');
});

it('should group thousands in the described ceiling', () => {
  expect(describeCapacity(1_200, 1_000)).toBe('Currently 1,200, with 1,000 free.');
});

it('should say when a full fleet has no free slots', () => {
  expect(describeCapacity(4, 0)).toBe('Currently 4, with no free slots left.');
});

// Zero is not "full", it is "closed", and an operator reading the field needs
// those apart.
it('should say when no seats are offered at all', () => {
  expect(describeCapacity(0, 0)).toBe('Currently 0 — no seats are offered.');
});
