// fixture.ts — input to `cofferdam check` for the NoHttpClient plugin (cd-b5h).
//
// Comments label the expected outcome on each line. The plugin should emit
// FOUR issues in total:
//   - FLAG IMPORT #1 (axios import on line 11)
//   - FLAG IMPORT #2 (node-fetch import on line 12)
//   - FLAG CALL #1   (axios.get usage on line 24)
//   - FLAG CALL #2   (myFetch alias usage on line 25)
//
// Every other occurrence is exempted by one of:
//   - allowlisted prefix in cofferdam.toml (e.g. "@org/http")
//   - engine-level `// cofferdam-ignore: NoHttpClient` directive
//   - the line is in a comment / not a real import or call

import axios from "axios";                          // FLAG IMPORT #1
import { fetch as myFetch } from "node-fetch";       // FLAG IMPORT #2
import { send } from "@org/http";                    // OK (allowlisted prefix)
import { createServer } from "node:http";            // OK (not in bannedSources)

// cofferdam-ignore: NoHttpClient: legacy migration path, see ROVI-512
import got from "got";                               // EXEMPT (engine suppression)

// Comment mentioning axios should not trigger anything.

export async function fetchUser(id: string): Promise<unknown> {
  const a = await axios.get(`/users/${id}`);          // FLAG CALL #1
  const b = await myFetch("/health");                 // FLAG CALL #2
  return send(a, b);                                  // OK (allowed module)
}

export async function fetchSafely(): Promise<unknown> {
  // cofferdam-ignore: NoHttpClient: backfill script reads raw axios
  return axios("/raw");                               // EXEMPT (engine suppression)
}
