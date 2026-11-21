// Validation and formatting for bug reports and feedback. Pure functions (no I/O) so they
// can be tested without a network; api/feedback.mjs does the HTTP side.

export const LIMITS = {
  message: 10000, // characters
  email: 254,
  system: 4000,
  log: 20000,
  version: 40,
  body: 64 * 1024, // bytes of JSON accepted at all
};

// `crash` reports are sent by the app itself on the launch after a crash (Settings › Privacy).
const KINDS = { bug: "Bug", feedback: "Feedback", crash: "Crash" };
const EMAIL_RE = /^[^\s@<>()[\]",;:]+@[^\s@<>()[\]",;:]+\.[^\s@<>()[\]",;:]{2,}$/;

/** Trims a string field; anything that is not a string becomes "". */
function str(v) {
  return typeof v === "string" ? v.replace(/\r\n?/g, "\n").trim() : "";
}

/**
 * Checks a submission. Returns `{ ok: true, report }` with the cleaned fields, or
 * `{ ok: false, error }` with a message the app can show. A filled honeypot field
 * (`website`, hidden in every form) returns `{ ok: true, spam: true }` so bots see success.
 */
export function validate(input) {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    return { ok: false, error: "The report was empty." };
  }
  if (str(input.website) !== "") return { ok: true, spam: true };
  const kind = str(input.kind);
  if (!(kind in KINDS)) return { ok: false, error: "Unknown report type." };
  const report = {
    kind,
    message: str(input.message),
    email: str(input.email),
    system: str(input.system),
    log: str(input.log),
    version: str(input.version),
  };
  if (report.message.length < 3) return { ok: false, error: "Please describe what happened." };
  for (const k of ["message", "email", "system", "log", "version"]) {
    if (report[k].length > LIMITS[k]) return { ok: false, error: `The ${k} field is too long.` };
  }
  if (report.email && !EMAIL_RE.test(report.email)) {
    return { ok: false, error: "That email address doesn't look right." };
  }
  return { ok: true, report };
}

/**
 * Stops user text from pinging people or linking issues when it is posted to GitHub:
 * `@name` and `#123` get a zero-width joiner after the sign.
 */
export function defang(text) {
  return text.replace(/@(?=[A-Za-z0-9-])/g, "@⁠").replace(/#(?=\d)/g, "#⁠");
}

/** A code fence longer than any backtick run inside `text`, so the text can't close it. */
function fence(text) {
  const longest = Math.max(2, ...(text.match(/`+/g) || []).map((r) => r.length));
  const f = "`".repeat(longest + 1);
  return `${f}text\n${text}\n${f}`;
}

/** Issue title: the first line of the message, cut to 72 characters. */
export function title(report) {
  const first = report.message.split("\n").find((l) => l.trim() !== "") || "";
  const clean = defang(first.trim().replace(/\s+/g, " "));
  const cut = clean.length > 72 ? `${clean.slice(0, 71).trimEnd()}…` : clean;
  return `[${KINDS[report.kind]}] ${cut}`;
}

/** Issue body (Markdown). `includeEmail` is false when the issues are public. */
export function body(report, { includeEmail = true } = {}) {
  const lines = [];
  lines.push(defang(report.message), "");
  lines.push("---", "");
  lines.push(`**Type:** ${KINDS[report.kind]}`);
  if (report.version) lines.push(`**App version:** ${defang(report.version)}`);
  if (report.email) {
    lines.push(includeEmail ? `**Reply to:** ${report.email}` : "**Reply to:** given (kept private)");
  } else {
    lines.push("**Reply to:** not given");
  }
  if (report.system) {
    lines.push("", "<details><summary>System info</summary>", "", fence(report.system), "", "</details>");
  }
  if (report.log) {
    lines.push("", "<details><summary>Recent log</summary>", "", fence(report.log), "", "</details>");
  }
  lines.push("", report.kind === "crash" ? "_Sent by the app after a crash._" : "_Sent from the in-app form._");
  return lines.join("\n");
}

export function labels(report) {
  return [report.kind === "feedback" ? "feedback" : report.kind, "from-app"];
}

/**
 * Fixed-window rate limit kept in memory. Instances are short-lived, so this only slows a
 * burst from one address; GitHub's own limits and the honeypot do the rest.
 */
export class RateLimiter {
  constructor({ max = 5, windowMs = 10 * 60 * 1000 } = {}) {
    this.max = max;
    this.windowMs = windowMs;
    this.hits = new Map();
  }
  allow(key, now = Date.now()) {
    const h = this.hits.get(key);
    if (!h || now - h.start >= this.windowMs) {
      this.hits.set(key, { start: now, count: 1 });
      if (this.hits.size > 10000) this.hits.clear();
      return true;
    }
    h.count += 1;
    return h.count <= this.max;
  }
}
