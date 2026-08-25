// Deliberate violation: a screen hiding its content behind a raw isError.
export const GatedPage = () => {
  const seats = useSeats();

  if (seats.isError) {
    return <p>Could not load the fleet.</p>;
  }

  return <p>{seats.data.seats.length} seats</p>;
};
