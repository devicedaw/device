#!/usr/bin/env bash
# Keeps one GitHub issue per scheduled workflow up to date (CI's nightly run, the weekly
# security check), so a failure that happens without a commit reaches someone:
#
#   FAILED=true|false TITLE="Nightly CI failed" tools/ci/scheduled-run-issue.sh
#
# FAILED=true: opens the issue (label `ci-nightly`) or, when one is open, comments on it with
# this run, so every failure notifies its subscribers. FAILED=false: comments that the run
# passed and closes the open issue. Needs GH_TOKEN (the workflow token with issues: write),
# and the GITHUB_* variables Actions sets.
set -euo pipefail

: "${FAILED:?FAILED=true or false}"
: "${TITLE:?TITLE=the issue title}"
label=ci-nightly
repo=$GITHUB_REPOSITORY
run_url="$GITHUB_SERVER_URL/$repo/actions/runs/$GITHUB_RUN_ID"

number=$(gh api "repos/$repo/issues?state=open&labels=$label&per_page=100" \
    --jq "[.[] | select(.pull_request == null and .title == \"$TITLE\")][0].number // empty")

if [ "$FAILED" != true ]; then
    if [ -n "$number" ]; then
        gh api -X POST "repos/$repo/issues/$number/comments" -f "body=Passed again: $run_url" --jq .html_url
        gh api -X PATCH "repos/$repo/issues/$number" -f state=closed -f state_reason=completed --jq .html_url
    fi
    exit 0
fi

# The jobs and steps that failed in this run (this job is still running, so it is not one of them).
failed=$(gh api --paginate "repos/$repo/actions/runs/$GITHUB_RUN_ID/attempts/${GITHUB_RUN_ATTEMPT:-1}/jobs?per_page=100" \
    --jq '.jobs[] | select(.conclusion == "failure" or .conclusion == "cancelled" or .conclusion == "timed_out")
          | "- \(.name) (\(.conclusion)): \([.steps[]? | select(.conclusion == "failure" or .conclusion == "cancelled") | .name] | join(", "))"')
body=$(printf '%s on %s (%s) failed: %s\n\n%s\n' "$GITHUB_WORKFLOW" "$GITHUB_REF_NAME" "${GITHUB_SHA:0:8}" "$run_url" "${failed:-"(no failed job found: see the run)"}")

if [ -n "$number" ]; then
    gh api -X POST "repos/$repo/issues/$number/comments" -f "body=$body" --jq .html_url
else
    gh label create "$label" --repo "$repo" --color B60205 \
        --description "A scheduled workflow run failed (opened and closed by the workflows)" --force
    {
        echo "$body"
        echo
        echo "_Opened by the $GITHUB_WORKFLOW workflow. Each later failure adds a comment; the next passing scheduled run closes this issue._"
    } > "$RUNNER_TEMP/issue.md"
    gh api -X POST "repos/$repo/issues" -f "title=$TITLE" -f "labels[]=$label" -F "body=@$RUNNER_TEMP/issue.md" --jq .html_url
fi
