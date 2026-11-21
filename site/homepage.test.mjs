// Checks the generated homepage agent cards in site/dist.
//   node build.mjs && node --test homepage.test.mjs
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const SITE_DIR = path.dirname(fileURLToPath(import.meta.url));
const homepage = await readFile(path.join(SITE_DIR, "dist", "index.html"), "utf8");

const cardRe = (label) =>
  new RegExp(`href="https://www\\.raitox\\.tech/everett/setup/${label}"[^>]*>.*?<small>([^<]*)</small>`, "s");

test("Pi card advertises the extension tool bridge, not a CLI fallback", () => {
  const card = homepage.match(cardRe("pi"));
  assert.ok(card, "Pi card missing from homepage");
  assert.equal(card[1], "Extension tool bridge · no native MCP");
});

test("other agent cards keep their labels", () => {
  const expected = {
    "claude-code": "Hooks + MCP",
    codex: "Hooks + MCP",
    omp: "Extension + MCP",
    hermes: "Manual MCP entry",
    "grok-cli": "Local CLI · not Grok Bot",
    "devin-cli": "Session hooks + MCP",
  };
  for (const [slug, label] of Object.entries(expected)) {
    const card = homepage.match(cardRe(slug));
    assert.ok(card, `${slug} card missing from homepage`);
    assert.equal(card[1], label);
  }
});
