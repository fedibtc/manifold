// Ambient type for CSS Module imports. The apps get this from `vite/client`;
// mock-devtools is typechecked with plain `tsc`, so it declares the shape here.
declare module '*.module.css' {
  const classes: Record<string, string>;
  export default classes;
}
