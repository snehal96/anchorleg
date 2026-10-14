# codex 0.153.4: approvals reviewed by Codex itself (`approvalsReviewer: auto_review`, 2026-10-12)

`auto-review.jsonl`: one turn over `codex app-server` (no experimental API), `thread/start` with
`approvalPolicy: "on-request"`, `sandbox: "workspace-write"` and `approvalsReviewer:
"auto_review"`. The prompt asked for a command that needs the network
(`curl -sI https://example.com | head -1`), which the sandbox would otherwise ask about. Model
`gpt-5.6-luna`, effort `low`; about 3 model calls (the turn and the reviewer).

- No approval request reached the client. Codex sent `item/autoApprovalReview/started`
  (`review.status: "inProgress"`), a `guardianWarning` with the decision in words, then
  `item/autoApprovalReview/completed` with `review: {status: "approved", riskLevel: "low",
  userAuthorization: "high", rationale}`, `action: {type: "command", command, cwd, source}` and
  `targetItemId` (the command's item id), and the command ran.
- Statuses in the schema: `inProgress`, `approved`, `denied`, `timedOut`, `aborted`; risk levels
  `low`, `medium`, `high`, `critical`. The schema marks the payload `[UNSTABLE]`.

Streaming deltas, rate-limit and hook messages were dropped; the rest went through
`scripts/scrub-capture.py`.
