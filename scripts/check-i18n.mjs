// Keeps the dictionaries honest:
//  - every language has exactly the keys and the {placeholders} of ru.json;
//  - every key the code asks for exists;
//  - no key is left that nothing asks for.
//
//   node scripts/check-i18n.mjs          checks and fails on any difference
//   node scripts/check-i18n.mjs --list   prints the keys the code asks for

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const LANGUAGES = ["ru", "en", "de", "es", "fr", "pt", "pl", "tr", "zh", "ja", "ko"];

function walk(folder, extensions, out = []) {
  for (const name of readdirSync(folder)) {
    const path = join(folder, name);
    if (statSync(path).isDirectory()) {
      walk(path, extensions, out);
    } else if (extensions.includes(extname(name))) {
      out.push(path);
    }
  }
  return out;
}

function flatten(node, prefix = "", out = new Map()) {
  for (const [key, value] of Object.entries(node)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") {
      out.set(path, value);
    } else if (value && typeof value === "object" && !Array.isArray(value)) {
      flatten(value, path, out);
    } else {
      throw new Error(`${path} is neither a string nor a group`);
    }
  }
  return out;
}

function placeholders(text) {
  return [...new Set([...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]))].sort().join(",");
}

// Texts that show braces as they are; the code passes no values for them.
const LITERAL_BRACES = new Set(["output.namingHint", "studio.stylePromptHint"]);

// What the code asks for.
const exact = new Set();
const prefixes = new Set();
// Values the code passes with a key: "a,b" per call.
const passed = new Map();
for (const file of walk(join(root, "src"), [".ts", ".tsx"])) {
  const source = readFileSync(file, "utf8");
  for (const match of source.matchAll(/\bt[d]?\(\s*"([A-Za-z][\w.-]*)"\s*(?:,\s*\{([^{}]*)\})?/g)) {
    exact.add(match[1]);
    const names = (match[2] ?? "")
      .split(",")
      .map((part) => part.trim().split(":")[0].trim())
      .filter((name) => /^\w+$/.test(name))
      .sort()
      .join(",");
    if (!passed.has(match[1])) {
      passed.set(match[1], new Set());
    }
    passed.get(match[1]).add(names);
  }
  for (const match of source.matchAll(/\b(?:td|translate)\(\s*(?:\w+,\s*)?`([A-Za-z][\w.]*\.)\$\{/g)) {
    prefixes.add(match[1]);
  }
  // Option lists built from a prefix: labelled("output.effortKind", [...]).
  for (const match of source.matchAll(/\blabelled\(\s*"([A-Za-z][\w.]*)"/g)) {
    prefixes.add(`${match[1]}.`);
  }
}
for (const file of walk(join(root, "src-tauri", "src"), [".rs"])) {
  const source = readFileSync(file, "utf8");
  for (const match of source.matchAll(/"(native\.[\w.]+|errors\.unknown)"/g)) {
    exact.add(match[1]);
  }
}
// The translator looks errors up by their code.
prefixes.add("errors.");

if (process.argv.includes("--list")) {
  console.log([...exact].sort().join("\n"));
  console.log("\nPrefixes:\n" + [...prefixes].sort().join("\n"));
  process.exit(0);
}

const problems = [];
const dictionaries = new Map();
for (const language of LANGUAGES) {
  try {
    const text = readFileSync(join(root, "src", "i18n", `${language}.json`), "utf8");
    dictionaries.set(language, flatten(JSON.parse(text.replace(/^\uFEFF/, ""))));
  } catch (error) {
    problems.push(`${language}.json: ${error.message}`);
  }
}

const base = dictionaries.get("ru");
if (base) {
  for (const [language, dictionary] of dictionaries) {
    for (const [key, text] of base) {
      const other = dictionary.get(key);
      if (other === undefined) {
        problems.push(`${language}: missing ${key}`);
      } else if (other.trim() === "") {
        problems.push(`${language}: empty ${key}`);
      } else if (placeholders(other) !== placeholders(text)) {
        problems.push(`${language}: placeholders of ${key} differ ({${placeholders(other)}} vs {${placeholders(text)}})`);
      }
    }
    for (const key of dictionary.keys()) {
      if (!base.has(key)) {
        problems.push(`${language}: unexpected ${key}`);
      }
    }
  }
  for (const key of exact) {
    if (!base.has(key)) {
      problems.push(`code asks for a missing key: ${key}`);
    }
  }
  for (const [key, calls] of passed) {
    const expected = base.get(key);
    if (expected === undefined || LITERAL_BRACES.has(key)) {
      continue;
    }
    for (const names of calls) {
      if (names !== placeholders(expected)) {
        problems.push(`code passes {${names}} to ${key}, the text expects {${placeholders(expected)}}`);
      }
    }
  }
  for (const prefix of prefixes) {
    if (![...base.keys()].some((key) => key.startsWith(prefix))) {
      problems.push(`code asks for keys under ${prefix} and there are none`);
    }
  }
  for (const key of base.keys()) {
    if (!exact.has(key) && ![...prefixes].some((prefix) => key.startsWith(prefix))) {
      problems.push(`nothing asks for ${key}`);
    }
  }
}

if (problems.length > 0) {
  console.error(problems.join("\n"));
  console.error(`\n${problems.length} problem(s) in the dictionaries.`);
  process.exit(1);
}
console.log(`Dictionaries are consistent: ${LANGUAGES.length} languages, ${base.size} strings each.`);
