#!/usr/bin/env node
// Release guard for "Windows installer (manual)" (PKG-4): refuses to publish when the tag v<version> already exists
// on another commit than the one that was built.
//
//   node scripts/release-guard.mjs [--tag v0.8.0] [--repo owner/name] [--commit <sha>] [--allow-overwrite true|false]
//
// Why: the publish step uploads with `gh release upload --clobber`. Without this check a run from another commit
// (a branch whose version was not bumped, a second dispatch after more commits) silently replaced the installer of
// a published Release while its tag kept pointing at the old source — this happened to v0.1.0.
//
//   --tag     default: v<version of src-tauri/tauri.conf.json>, the tag the workflow publishes to
//   --repo    default: $GITHUB_REPOSITORY
//   --commit  default: `git rev-parse HEAD`, the checkout that is being built
//
// Exit code 0: the tag does not exist yet (the workflow creates the Release at the built commit), or the tag
//              points at the built commit (same source: adding the other variant, repeating an upload), or it
//              points elsewhere and `--allow-overwrite true` was given (a warning says what is being replaced).
// Exit code 1: the tag points at another commit, or the tag could not be read from the GitHub API.
//
// Read-only. Uses the GitHub CLI (`gh api`; GH_TOKEN on Actions) with the REST endpoints
// git/matching-refs/tags/<tag> (an empty list means "no such tag", so nothing depends on error texts) and
// git/tags/<sha> (peels an annotated tag).
//
// The script always runs its check when started: there is deliberately no "only when executed directly" test,
// because a guard that could end up doing nothing would exit 0 and let the publish through.
import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ON_ACTIONS = process.env.GITHUB_ACTIONS === "true";
const SHA = /^[0-9a-f]{40}$/;

function parseArgs(argv) {
  const value = (name) => {
    const eq = argv.find((a) => a.startsWith(`--${name}=`));
    if (eq !== undefined) return eq.slice(name.length + 3);
    const i = argv.indexOf(`--${name}`);
    if (i < 0) return undefined;
    const next = argv[i + 1];
    return next === undefined || next.startsWith("--") ? "" : next;
  };
  return {
    tag: value("tag") || null,
    repo: value("repo") || process.env.GITHUB_REPOSITORY || null,
    commit: value("commit") || null,
    // Only the literal value "true" allows it. A bare flag, "false" or "" (an unset workflow input, or an empty
    // argument that the shell dropped) all mean no — the override must never switch on by accident.
    allowOverwrite: value("allow-overwrite") === "true",
  };
}

const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

/** `gh api <path>` → parsed JSON. Retries transient failures; throws after the last attempt. */
function ghApi(apiPath, attempts = 3) {
  let last = "";
  for (let attempt = 1; attempt <= attempts; attempt++) {
    const r = spawnSync("gh", ["api", apiPath], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
    if (r.error) throw new Error(`could not run the GitHub CLI (gh): ${r.error.message}`);
    if (r.status === 0) {
      try {
        return JSON.parse(r.stdout);
      } catch {
        last = `gh api ${apiPath}: the answer is not JSON`;
      }
    } else last = `gh api ${apiPath}: ${(r.stderr || r.stdout || `exit code ${r.status}`).trim().split(/\r?\n/)[0]}`;
    if (attempt < attempts) sleep(2000 * attempt);
  }
  throw new Error(last);
}

/** The commit `refs/tags/<tag>` points at, or null when the tag does not exist. */
function tagCommit(repo, tag) {
  const refs = ghApi(`repos/${repo}/git/matching-refs/tags/${tag}`);
  if (!Array.isArray(refs)) throw new Error("unexpected answer from git/matching-refs");
  const ref = refs.find((r) => r?.ref === `refs/tags/${tag}`); // the endpoint matches by prefix (v0.8.0 → v0.8.0-rc1 too)
  if (!ref) return null;
  let object = ref.object;
  for (let hop = 0; object?.type === "tag" && hop < 5; hop++) object = ghApi(`repos/${repo}/git/tags/${object.sha}`)?.object;
  if (object?.type !== "commit" || !SHA.test(object.sha ?? "")) throw new Error(`refs/tags/${tag} does not resolve to a commit`);
  return object.sha;
}

/** Decides from the built commit and the commit the tag points at (null = no such tag). */
function decide({ tag, built, tagged, allowOverwrite }) {
  if (tagged === null) return { ok: true, level: "info", message: `tag ${tag} does not exist yet — a new Release will be created at ${built}` };
  if (tagged === built) return { ok: true, level: "info", message: `tag ${tag} already points at the built commit ${built} — same source, assets may be added or replaced` };
  const what = `tag ${tag} points at ${tagged}, but this run builds ${built}`;
  if (allowOverwrite) {
    return { ok: true, level: "warning", message: `${what}; allow_overwrite is set, so the assets of that Release are replaced by a build of another commit (the tag is not moved)` };
  }
  return {
    ok: false,
    level: "error",
    message: `${what}. Publishing would replace the assets of a published Release with a build of other source. Bump the version (a new tag), build the tagged commit, or dispatch again with allow_overwrite=true to replace them on purpose.`,
  };
}

function say(level, message) {
  if (ON_ACTIONS && level !== "info") console.log(`::${level} title=Release guard::${message}`);
  else console.log(`release guard: ${level === "info" ? "" : `${level.toUpperCase()}: `}${message}`);
}

function main(argv) {
  const args = parseArgs(argv);
  const tag = args.tag ?? `v${JSON.parse(readFileSync(path.join(ROOT, "src-tauri", "tauri.conf.json"), "utf8")).version}`;
  if (!/^[A-Za-z0-9][A-Za-z0-9._+-]*$/.test(tag)) throw new Error(`unusable tag name ${JSON.stringify(tag)}`);
  if (!args.repo || !/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(args.repo)) throw new Error("no repository: pass --repo owner/name or set GITHUB_REPOSITORY");
  let built = args.commit;
  if (!built) {
    const r = spawnSync("git", ["rev-parse", "HEAD"], { cwd: ROOT, encoding: "utf8" });
    built = (r.stdout ?? "").trim();
  }
  if (!SHA.test(built)) throw new Error(`the built commit is not a full SHA (${JSON.stringify(built)}); pass --commit or run inside the checkout`);
  const tagged = tagCommit(args.repo, tag);
  const verdict = decide({ tag, built, tagged, allowOverwrite: args.allowOverwrite });
  say(verdict.level, verdict.message);
  return verdict.ok ? 0 : 1;
}

// Any failure to reach a verdict is a refusal (exit code 1), never a pass. The exit code is set before the check
// runs, and process.exitCode (not process.exit) lets the output drain when stdout is a pipe.
process.exitCode = 1;
try {
  process.exitCode = main(process.argv.slice(2));
} catch (e) {
  say("error", `${e.message} — refusing to publish because the tag could not be checked`);
}
