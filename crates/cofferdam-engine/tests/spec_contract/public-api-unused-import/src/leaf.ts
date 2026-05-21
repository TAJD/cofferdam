// NOT in [public_api].exports. This re-export of BETA has no
// project-internal consumer, so Warning.UnusedImport MUST flag it.
// Same symbol re-exported through the allow-listed index.ts above is
// silent — only the leaf re-export should fire.
export { BETA } from "./internal";
