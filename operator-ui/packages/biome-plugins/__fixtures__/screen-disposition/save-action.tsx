// Out of scope on purpose: a mutation's isError is an action that failed, not a
// read the screen has to dispose of. This must NOT be flagged.
export const SaveAction = () => {
  const setPrice = useSetPrice();

  return (
    <>
      {setPrice.isError && <Banner variant="error">{describeActionError(setPrice.error)}</Banner>}

      <Button onClick={handleSave}>Save</Button>
    </>
  );
};
