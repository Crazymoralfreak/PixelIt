// Writes src/licenses.json: every third-party library that ends up in the
// Windows build, with its version and license, as the "About" page shows
// them. The Rust side comes from `cargo metadata`, the web side from the
// installed packages.
//
//   node scripts/make-licenses.mjs          writes the file
//   node scripts/make-licenses.mjs --check  fails when the file is out of date

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const target = join(root, "src", "licenses.json");

function rustLibraries() {
  const output = execFileSync(
    "cargo",
    ["metadata", "--format-version", "1", "--locked", "--filter-platform", "x86_64-pc-windows-msvc"],
    { cwd: root, maxBuffer: 256 * 1024 * 1024, encoding: "utf8" },
  );
  const metadata = JSON.parse(output);
  const own = new Set(metadata.workspace_members);
  // Only what the resolver reaches: build scripts and tests included, other platforms left out.
  const reached = new Set(metadata.resolve.nodes.map((node) => node.id));
  return metadata.packages
    .filter((item) => reached.has(item.id) && !own.has(item.id))
    .map((item) => ({
      name: item.name,
      version: item.version,
      license: item.license ?? (item.license_file ? "see the license file of the package" : "unknown"),
      kind: "rust",
    }));
}

function webLibraries() {
  const manifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  return Object.keys(manifest.dependencies ?? {}).flatMap((name) => {
    const path = join(root, "node_modules", name, "package.json");
    if (!existsSync(path)) {
      return [];
    }
    const item = JSON.parse(readFileSync(path, "utf8"));
    return [{ name: item.name, version: item.version, license: item.license ?? "unknown", kind: "web" }];
  });
}

const libraries = [...rustLibraries(), ...webLibraries()].sort(
  (a, b) => a.name.localeCompare(b.name, "en") || a.version.localeCompare(b.version, "en"),
);
const text = `${JSON.stringify(libraries, null, 1)}\n`;

if (process.argv.includes("--check")) {
  const current = existsSync(target) ? readFileSync(target, "utf8").replace(/\r\n/g, "\n") : "";
  if (current !== text) {
    console.error("src/licenses.json is out of date: run `node scripts/make-licenses.mjs`.");
    process.exit(1);
  }
  console.log(`src/licenses.json is up to date (${libraries.length} libraries).`);
} else {
  writeFileSync(target, text);
  console.log(`Wrote ${libraries.length} libraries to src/licenses.json.`);
}
