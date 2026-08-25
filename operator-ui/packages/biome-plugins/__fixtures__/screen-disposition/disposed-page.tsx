// The shape the rule asks for: one disposition over the screen's reads, and the
// claims rendered through the surface.
export const DisposedPage = () => {
  const seats = useSeats();
  const { disposition, retry } = useQueryDisposition([seats]);

  return (
    <QuerySurface disposition={disposition} onRetry={retry}>
      <p>{seats.data?.seats.length ?? 0} seats</p>
    </QuerySurface>
  );
};
