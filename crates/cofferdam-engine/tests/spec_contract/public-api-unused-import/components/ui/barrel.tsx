// Allow-listed via the `components/ui/**` glob entry — the named
// re-export must NOT be flagged by Warning.UnusedImport even though
// nothing else in the corpus imports `Wrapper`.
export { Wrapper } from "./wrapper";
