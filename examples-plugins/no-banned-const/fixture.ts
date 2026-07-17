// fixture.ts — input to `cofferdam check` for the NoBannedConst plugin (CD-78).
//
// The gap CD-78 closes: a `const`-bound function is invisible to
// findAll("Function") but visible to findAll("VariableDeclaration").
// The plugin should emit TWO issues:
//   - FLAG #1 (const buildHeaders arrow on line 15)
//   - FLAG #2 (export const fetch arrow on line 23)
//
// Every other declaration is exempt because it's not a `const`, its name
// isn't banned, or it's a destructuring pattern with no simple name.

function buildHeadersFn() {                          // OK — plain function decl, name not banned
  return {};
}

const buildHeaders = (token: string) => ({           // FLAG #1 — const-bound banned identifier
  Authorization: token,
});

let buildHeaders2 = () => ({});                      // OK — `let`, not `const`

const { header, footer } = { header: 1, footer: 2 }; // OK — destructuring, no simple name

const helper = function () {                          // OK — const-bound but name not banned
  return 1;
};

export const fetch = async () => ({ ok: true });     // FLAG #2 — const-bound banned identifier

void buildHeadersFn;
void buildHeaders;
void buildHeaders2;
void header;
void footer;
void helper;
