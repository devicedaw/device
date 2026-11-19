# Feedback service

Receives "Report a bug" and "Share your feedback" submissions from the app (`crates/dstudio-app/src/feedback.rs`)
and files each one as an issue in a private repository, where the maintainer reads and triages them. It is one
Vercel function with no dependencies; the app and the website never hold a credential.

| File | What it does |
| --- | --- |
| `api/feedback.mjs` | `POST /api/feedback`: rate limit, validation, GitHub issue |
| `lib/report.mjs` | Validation, issue title and body, the rate limiter (pure, tested) |
| `test/report.test.mjs` | `node --test services/feedback/test/*.test.mjs` (also run by CI's lint job) |

## Request

```json
{ "kind": "bug", "message": "What happened", "email": "", "version": "0.2.0", "system": "", "log": "" }
```

`kind` is `bug` or `feedback`; `message` is required (3 to 10,000 characters); `email` is optional. `system` and
`log` are only filled when the reporter leaves "Include system info and recent activity" on. A non-empty `website`
field is a honeypot: the request is answered with success and dropped. Responses are `{ "ok": true }` or
`{ "error": "<sentence the app shows>" }` with status 400, 429 or 502.

## Abuse and privacy

- At most 5 reports per address per 10 minutes per instance, 64 KB per request, field length limits.
- `@mentions` and `#123` references in user text are defanged so a report cannot ping anyone; system info and logs go
  in collapsed code blocks that user text cannot close.
- Reporter emails appear only in the private issue. Set `INCLUDE_EMAIL=false` if the issues ever go to a public
  repository.
- CORS allows only `https://devicedaw.github.io`, so the website can post too.

## Setup (Vercel project `device-feedback`)

Environment variables (Production):

- `FEEDBACK_REPO`: `devicedaw/feedback`
- `GITHUB_TOKEN`: a fine-grained token, resource owner devicedaw, only that repository, permission Issues read and
  write. Rotate it there; nothing else changes.

The app posts to `https://device-feedback.vercel.app/api/feedback`; set `DSTUDIO_FEEDBACK_URL` to point a build at
another deployment (for example `vercel dev` on `http://localhost:3000/api/feedback`).
