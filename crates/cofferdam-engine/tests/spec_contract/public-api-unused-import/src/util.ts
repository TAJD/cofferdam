// Default export consumed only by the allow-listed `src/index.ts`
// re-export above. Confirms that even the SOURCE file's `default`
// export doesn't get spuriously flagged when its sole consumer is a
// public-api re-exporter (the default re-export through index.ts
// counts as consumption, same as for a non-public-api re-exporter).
const helper = { kind: "util" } as const;
export default helper;
