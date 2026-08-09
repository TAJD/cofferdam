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

// FLAGGED: American spellings, outnumbered by the British ones above.
// Initializes the color table and normalizes the behavior of the analyzer.
export function initialize(): void {
  const message = 'the analyzer keeps every artifact it finds';
  console.log(message);
}

// NOT FLAGGED: identifiers are never read, so these API names are safe.
export const serializeCatalog = serialize;
export const initializeColorTable = initialize;

// NOT FLAGGED: a literal with no space is a code token, not prose.
export const MODULE = './normalize';
export const KEY = 'color';
