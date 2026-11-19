// POST /api/feedback: files a bug report or feedback from Device Studio (or the website)
// as a GitHub issue. Configuration (Vercel project environment variables):
//   GITHUB_TOKEN    fine-grained token with Issues: read and write on FEEDBACK_REPO only
//   FEEDBACK_REPO   owner/name of the repository the issues go to (e.g. devicedaw/feedback)
//   INCLUDE_EMAIL   "false" leaves reporter emails out of the issue (use for a public repo)
import { LIMITS, RateLimiter, body, labels, title, validate } from "../lib/report.mjs";

const limiter = new RateLimiter();
const ALLOWED_ORIGINS = new Set(["https://devicedaw.github.io"]);

function cors(req, res) {
  const origin = req.headers.origin;
  if (origin && ALLOWED_ORIGINS.has(origin)) {
    res.setHeader("Access-Control-Allow-Origin", origin);
    res.setHeader("Vary", "Origin");
    res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
    res.setHeader("Access-Control-Allow-Headers", "Content-Type");
    res.setHeader("Access-Control-Max-Age", "86400");
  }
}

function send(res, status, payload) {
  res.statusCode = status;
  res.setHeader("Content-Type", "application/json");
  res.setHeader("Cache-Control", "no-store");
  res.end(JSON.stringify(payload));
}

async function readJson(req) {
  if (req.body !== undefined && typeof req.body === "object" && req.body !== null) return req.body;
  let size = 0;
  const chunks = [];
  for await (const chunk of req) {
    size += chunk.length;
    if (size > LIMITS.body) throw new Error("too large");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

async function createIssue(report) {
  const repo = process.env.FEEDBACK_REPO;
  const token = process.env.GITHUB_TOKEN;
  if (!repo || !token) throw new Error("not configured");
  const includeEmail = process.env.INCLUDE_EMAIL !== "false";
  const post = (withLabels) =>
    fetch(`https://api.github.com/repos/${repo}/issues`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "device-feedback",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        title: title(report),
        body: body(report, { includeEmail }),
        ...(withLabels ? { labels: labels(report) } : {}),
      }),
    });
  let r = await post(true);
  // A token without permission to create labels gets 403/422 for unknown labels: file it plain.
  if (r.status === 403 || r.status === 422) r = await post(false);
  if (!r.ok) throw new Error(`GitHub answered ${r.status}`);
  const issue = await r.json();
  return issue.number;
}

export default async function handler(req, res) {
  cors(req, res);
  if (req.method === "OPTIONS") return send(res, 204, {});
  if (req.method !== "POST") return send(res, 405, { error: "Use POST." });
  const ip = String(req.headers["x-forwarded-for"] || "").split(",")[0].trim() || "unknown";
  if (!limiter.allow(ip)) return send(res, 429, { error: "Too many reports in a short time. Please try again later." });
  let input;
  try {
    input = await readJson(req);
  } catch {
    return send(res, 400, { error: "The report could not be read." });
  }
  const v = validate(input);
  if (!v.ok) return send(res, 400, { error: v.error });
  if (v.spam) return send(res, 200, { ok: true });
  try {
    await createIssue(v.report);
  } catch (e) {
    console.error("feedback: could not file issue:", e.message);
    return send(res, 502, { error: "The report couldn't be delivered right now. Please try again later." });
  }
  return send(res, 200, { ok: true });
}
