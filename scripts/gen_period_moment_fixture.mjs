// Generate tests/fixtures/periods/moment_iso_weeks.tsv from the vendored
// Moment: every date from Dec 20 to Jan 10 for 2000–2100, plus every
// Monday. Columns: YYYYMMDD → GGGG/GGGG-[W]WW.
//
// Run: node scripts/gen_period_moment_fixture.mjs

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const require = createRequire(import.meta.url);
const momentPath = join(
  root,
  "src",
  "native",
  "dataview",
  "tasks",
  "vendor",
  "moment.min.js",
);

const source = readFileSync(momentPath, "utf8");
// The vendored bundle assigns a global `moment`; evaluate it in a
// sandbox object so the fixture never depends on network or install.
const sandbox = {};
const factory = new Function("module", "exports", "window", "global", source);
const moduleShim = { exports: {} };
factory.call(
  sandbox,
  moduleShim,
  moduleShim.exports,
  sandbox,
  sandbox,
);
const moment =
  moduleShim.exports?.default ??
  moduleShim.exports ??
  sandbox.moment;
if (typeof moment !== "function") {
  throw new Error(`could not load moment from ${momentPath}`);
}

const days = new Map();
const start = new Date(Date.UTC(2000, 0, 1));
const end = new Date(Date.UTC(2100, 11, 31));
for (
  let cursor = new Date(start);
  cursor <= end;
  cursor.setUTCDate(cursor.getUTCDate() + 1)
) {
  const month = cursor.getUTCMonth();
  const day = cursor.getUTCDate();
  const weekday = cursor.getUTCDay();
  const inEdgeWindow =
    (month === 11 && day >= 20) || (month === 0 && day <= 10);
  const isMonday = weekday === 1;
  if (inEdgeWindow || isMonday) {
    const stamp = moment([
      cursor.getUTCFullYear(),
      month,
      day,
    ]);
    const yyyymmdd = stamp.format("YYYYMMDD");
    const weekPath = stamp.format("GGGG/GGGG-[W]WW");
    days.set(yyyymmdd, weekPath);
  }
}

const rows = [...days.entries()]
  .sort((a, b) => (a[0] < b[0] ? -1 : 1))
  .map(([day, weekPath]) => `${day}\t${weekPath}`);
const outDir = join(root, "tests", "fixtures", "periods");
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, "moment_iso_weeks.tsv"), `${rows.join("\n")}\n`);
console.log(`wrote ${rows.length} moment rows`);
