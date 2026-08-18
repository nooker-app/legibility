#!/usr/bin/env node
// Run Readability.js over a set of pages and emit what it produced, as JSON on stdout.
//
// # No dependencies, on purpose
//
// `Readability.js` and `JSDOMParser.js` come from the pinned `corpus/readability` submodule, so this
// runs the *exact* version the corpus was generated with and there is nothing to `npm install`. A
// PR never needs Node: the output is committed.
//
// The pipeline mirrors mozilla's own `test/generate-testcase.js` line for line — same fake base URI,
// same `classesToPreserve`, and `JSDOMParser` rather than jsdom, which is what they parse with. Any
// deviation here would make the comparison a comparison of harnesses.
"use strict";

const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "../../corpus/readability");
const Readability = require(path.join(ROOT, "Readability.js"));
const JSDOMParser = require(path.join(ROOT, "JSDOMParser.js"));

const URI = "http://fakehost/test/page.html";

/** Text of an HTML fragment, whitespace-collapsed — the same shape our metric scores. */
function textOf(html) {
  return (html || "")
    .replace(/<(script|style)\b[\s\S]*?<\/\1>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&")
    .split(/\s+/)
    .filter(Boolean)
    .join(" ");
}

function run(source) {
  const doc = new JSDOMParser().parse(source, URI);
  let result = null;
  let error = null;
  try {
    result = new Readability(doc, { classesToPreserve: ["caption"] }).parse();
  } catch (ex) {
    error = String((ex && ex.message) || ex);
  }
  if (!result) {
    // Readability returning null is an answer, not a crash: it is how it says "no article".
    return { ok: false, error, title: null, byline: null, text: "", length: 0 };
  }
  return {
    ok: true,
    error,
    title: result.title || null,
    byline: result.byline || null,
    siteName: result.siteName || null,
    publishedTime: result.publishedTime || null,
    lang: result.lang || null,
    // `textContent` rather than `content`: the metric scores text, and storing every article's HTML
    // would add megabytes to a permanent history for bytes nothing reads.
    text: textOf(result.content),
    length: result.length || 0,
  };
}

// `--out <path>` rather than stdout: an interactive shell profile (nvm, in this case) can print a
// banner into the same stream, and the corrupted JSON that results is a confusing way to learn that.
const argv = process.argv.slice(2);
let outPath = null;
const outAt = argv.indexOf("--out");
if (outAt !== -1) {
  outPath = argv[outAt + 1];
  argv.splice(outAt, 2);
}
const files = argv;
if (!files.length) {
  console.error("usage: run.js [--out result.json] <file.html>...   (or --corpus)");
  process.exit(2);
}

const out = {};
if (files[0] === "--corpus") {
  const dir = path.join(ROOT, "test/test-pages");
  for (const name of fs.readdirSync(dir).sort()) {
    const src = path.join(dir, name, "source.html");
    if (!fs.existsSync(src)) continue;
    out[name] = run(fs.readFileSync(src, "utf8"));
  }
} else {
  for (const f of files) {
    out[path.basename(f, ".html")] = run(fs.readFileSync(f, "utf8"));
  }
}
const json = JSON.stringify(out, null, 1) + "\n";
if (outPath) {
  fs.writeFileSync(outPath, json);
  console.error(`wrote ${Object.keys(out).length} page(s) to ${outPath}`);
} else {
  process.stdout.write(json);
}
