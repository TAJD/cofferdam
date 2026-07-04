#!/usr/bin/env node
// One-command release prep. Does every mechanical step between "main is green"
// and "cut the tag", in the right order, so none get forgotten:
//
//   node scripts/release.mjs X.Y.Z
//
//   1. refuse to run on a dirty tree (the release commit must be just the bump)
//   2. bump all version locations + regenerate the generated ones
//      (delegates to scripts/version.mjs set X.Y.Z --regen)
//   3. roll the CHANGELOG: [Unreleased] entries become the dated [X.Y.Z]
//      section, [Unreleased] is left empty
//   4. re-assert every version location agrees on X.Y.Z
//   5. print the review / commit / tag commands (git is left to you on purpose —
//      review the diff, and the tag push is the irreversible publish step)
//
// The tag push then drives .github/workflows/release.yml, whose `verify` job
// re-checks both the version locations and this CHANGELOG roll before anything
// is built or published.
//
// Zero deps: Node stdlib only, matching scripts/version.mjs.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const CHANGELOG = join(REPO_ROOT, "CHANGELOG.md");
const SEMVER_RE = /^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$/;

function die(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

function git(...args) {
  return execFileSync("git", args, { cwd: REPO_ROOT, encoding: "utf8" }).trim();
}

function node(...args) {
  execFileSync(process.execPath, args, { cwd: REPO_ROOT, stdio: "inherit" });
}

const version = process.argv[2];
const allowEmpty = process.argv.includes("--allow-empty-changelog");

if (!version || !SEMVER_RE.test(version)) {
  die("usage: node scripts/release.mjs X.Y.Z [--allow-empty-changelog]");
}

// 1. clean tree — the release commit should contain nothing but the bump
if (git("status", "--porcelain")) {
  die(
    "working tree is dirty. Commit or stash first so the release commit is just the version bump."
  );
}

// 3-prep. read + validate the CHANGELOG before mutating anything
let changelog = readFileSync(CHANGELOG, "utf8");
if (changelog.includes(`## [${version}]`)) {
  die(`CHANGELOG.md already has a ## [${version}] section — nothing to roll.`);
}
const unreleasedRe = /## \[Unreleased\]\n([\s\S]*?)(?=\n## \[)/;
const unreleasedMatch = changelog.match(unreleasedRe);
if (!unreleasedMatch) {
  die("could not find a '## [Unreleased]' section followed by a released one in CHANGELOG.md.");
}
if (!unreleasedMatch[1].trim() && !allowEmpty) {
  die(
    "## [Unreleased] is empty — you'd release with no notes. Add entries, or pass --allow-empty-changelog."
  );
}

// 2. version bump + regenerate generated locations
console.log(`\n==> bumping version to ${version} (+ regen)`);
node("scripts/version.mjs", "set", version, "--regen");

// 3. roll the CHANGELOG: insert a dated heading directly under [Unreleased]
const today = new Date().toISOString().slice(0, 10);
changelog = readFileSync(CHANGELOG, "utf8"); // re-read in case regen touched it
changelog = changelog.replace(
  "## [Unreleased]\n",
  `## [Unreleased]\n\n## [${version}] - ${today}\n`
);
writeFileSync(CHANGELOG, changelog);
console.log(`==> rolled CHANGELOG: [Unreleased] -> [${version}] - ${today}`);

// 4. re-assert consistency
console.log(`\n==> verifying all version locations agree on ${version}`);
node("scripts/version.mjs", "check", version);

// 5. hand off to the human for the git + tag steps
console.log(`
==> release ${version} prepared. Next, after reviewing the diff:

    git diff
    git commit -am "chore(release): prepare v${version}"
    git push origin main
    # wait for CI to go green, then cut the tag (this publishes to npm + GitHub):
    git tag -a v${version} -m "cofferdam ${version}"
    git push origin v${version}
`);
