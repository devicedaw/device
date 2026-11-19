import assert from "node:assert/strict";
import { test } from "node:test";
import { RateLimiter, body, defang, title, validate } from "../lib/report.mjs";

const bug = { kind: "bug", message: "Crash when I drag a clip\nSteps: drag it", email: "a@b.co", version: "0.2.0" };

test("valid bug report passes and is trimmed", () => {
  const v = validate({ ...bug, message: `  ${bug.message}  ` });
  assert.equal(v.ok, true);
  assert.equal(v.report.message, bug.message);
});

test("rejects bad input with a readable error", () => {
  assert.equal(validate(null).ok, false);
  assert.equal(validate({ ...bug, kind: "other" }).ok, false);
  assert.equal(validate({ ...bug, message: " " }).ok, false);
  assert.match(validate({ ...bug, email: "not an email" }).error, /email/);
  assert.match(validate({ ...bug, log: "x".repeat(20001) }).error, /too long/);
});

test("honeypot submissions are accepted silently", () => {
  assert.deepEqual(validate({ ...bug, website: "http://spam" }), { ok: true, spam: true });
});

test("user text cannot mention people, link issues or break out of fences", () => {
  assert.equal(defang("hi @chris see #12"), "hi @⁠chris see #⁠12");
  const b = body({ ...bug, system: "```\n@x", log: "" }, { includeEmail: true });
  assert.ok(b.includes("````text\n```\n@x\n````"));
});

test("title uses the first line, cut to 72 characters", () => {
  assert.equal(title(validate(bug).report), "[Bug] Crash when I drag a clip");
  const long = title({ kind: "feedback", message: "a ".repeat(100) });
  assert.ok(long.startsWith("[Feedback] ") && long.length <= 11 + 72);
});

test("email stays out of the body when issues are public", () => {
  const r = validate(bug).report;
  assert.ok(body(r).includes("a@b.co"));
  assert.ok(!body(r, { includeEmail: false }).includes("a@b.co"));
});

test("rate limiter allows a burst of five per window", () => {
  const l = new RateLimiter({ max: 5, windowMs: 1000 });
  for (let i = 0; i < 5; i++) assert.ok(l.allow("ip", 0));
  assert.equal(l.allow("ip", 10), false);
  assert.ok(l.allow("ip", 1000));
});
