import assert from "node:assert/strict";
import React from "react";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { MessageContent } from "../src/MessageContent";
import { ToolActivity } from "../src/ToolActivity";
import { ProviderModelSchema } from "../src/protocol";

test("model output renders GFM tables and code without executing HTML", () => {
  const text = '## Results\n\n| Model | Result |\n| --- | --- |\n| Worker | Ready |\n\n```python\nprint("hello")\n```\n\n<script>attack()</script>';
  const html = renderToStaticMarkup(<MessageContent text={text} locale="en" />);
  assert.match(html, /<h2>Results<\/h2>/);
  assert.match(html, /<table>/);
  assert.match(html, /language-python/);
  assert.doesNotMatch(html, /<script|attack\(\)/);
});

test("model output cannot embed remote tracking images or dangerous links", () => {
  const text = '![tracking](https://example.com/pixel) [unsafe](javascript:alert%281%29) [source](https://example.com/report) [secret](https://user:pass@example.com/)';
  const html = renderToStaticMarkup(<MessageContent text={text} locale="tr" />);
  assert.doesNotMatch(html, /<img|href="javascript:|href="https:\/\/user:/);
  assert.match(html, /href="https:\/\/example.com\/report"/);
  assert.match(html, /noreferrer noopener/);
});

test("tool activity changes language when the UI language changes", () => {
  const items = ["calculator · running", "web_research · completed_sources:3", "json_format · retry:2", "tools · unsupported"];
  const en = renderToStaticMarkup(<ToolActivity items={items} locale="en" />);
  const tr = renderToStaticMarkup(<ToolActivity items={items} locale="tr" />);
  assert.match(en, /Running/);
  assert.match(en, /3 sources/);
  assert.match(en, /does not support tool calls/);
  assert.match(tr, /Çalışıyor/);
  assert.match(tr, /3 kaynak/);
  assert.match(tr, /2. deneme/);
});

test("managed catalogs preserve load metadata and accept older provider responses", () => {
  assert.equal(ProviderModelSchema.parse({ id: "m", owned_by: null }).load_via, undefined);
  assert.equal(ProviderModelSchema.parse({ id: "m", owned_by: "LM Studio", load_via: "lm_studio", loaded: false }).loaded, false);
  assert.equal(ProviderModelSchema.safeParse({ id: "m", owned_by: null, load_via: "shell" }).success, false);
});
