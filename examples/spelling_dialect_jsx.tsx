// This fixture leans British, so the American spellings further down are
// the minority the check should flag.

// Normalises the colour palette before rendering. The behaviour is
// deliberate: an unrecognised colour is dropped rather than defaulted, and
// the catalogue keeps whatever order the analyser saw.
export function normalize(colours: string[]): string[] {
  return colours.filter((c) => c.startsWith('#'));
}

// Serialises the catalogue. Nothing here is centred on a colour value; the
// behaviour of the analyser is to keep every artefact it recognises.
export function serialize(catalogue: string[]): string {
  return catalogue.join(',');
}

// NOT FLAGGED: a className value is a space-separated class list, not
// prose, even when it contains a word that (out of context) is a dialect
// spelling on its own — `color-swatch` is not hyphen-adjacent to a
// recognized utility root, so only the positional className exclusion
// keeps it out.
export function Panel(): JSX.Element {
  return (
    <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between color-swatch">
      <p>{'the analyzer keeps every artifact it finds'}</p>
    </div>
  );
}
