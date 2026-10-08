// anchorleg-mod: runs inside each Claude session that anchorleg starts (Phase 2, D8).
//
// - session.measure: sends the session's exact quota to `anchorleg report --json`.
// - turn.step: before each model request after the first, if any window is at or above
//   ANCHORLEG_STOP_AT, records a stop for anchorleg and ends the turn without making the request.
//   The previous step's tool results are already in the transcript, so the next account can
//   resume from a clean point. anchorleg then switches accounts.
// - tool.check: when Claude would ask the person about a tool call (in `-p` that ask becomes a
//   refusal), asks anchorleg instead (`anchorleg permission ask`) and waits for the person's answer in
//   `anchorleg ui` or `anchorleg permission answer`. Nothing is allowed without that answer; if anchorleg
//   can't be reached the CLI's own verdict stands.
//
// It never touches credentials (D8): anchorleg does the switch by relaunching the CLI.
// Outside anchorleg (no ANCHORLEG_ACCOUNT) it does nothing.

import type { Register, SessionRateLimit } from 'claude-code'

type Reading = { window: string; used: number; resets_at?: number }

/** What `anchorleg permission ask --json` prints. */
type Answer = { decision?: string; id?: number; reason?: string }

/** `percentUsed` (0–100) and ISO `resetsAt` → anchorleg's fraction and Unix seconds. */
export function toReadings(limits: readonly SessionRateLimit[]): Reading[] {
  return limits.map((l) => {
    const reading: Reading = { window: l.kind, used: Math.min(Math.max(l.percentUsed / 100, 0), 1) }
    const t = l.resetsAt ? Date.parse(l.resetsAt) : NaN
    if (!Number.isNaN(t)) reading.resets_at = Math.floor(t / 1000)
    return reading
  })
}

/** The fullest window, as a fraction. */
export function maxUsed(limits: readonly SessionRateLimit[]): number {
  return limits.reduce((m, l) => Math.max(m, l.percentUsed / 100), 0)
}

export const register: Register = (on) => {
  let stopped = false

  on('session.measure', async ($, e, next) => {
    const result = await next(e)
    const account = await $.env.get('ANCHORLEG_ACCOUNT')
    const bin = await $.env.get('ANCHORLEG_BIN')
    if (!account || !bin || e.rateLimits.length === 0) return result
    const report = { account, readings: toReadings(e.rateLimits) }
    const run = await $.process.run([bin, 'report', '--json'], {
      stdin: JSON.stringify(report),
      timeoutMs: 10_000,
    })
    if (run.exitCode !== 0) $.ui.log(`anchorleg report failed: ${run.stderr.trim()}`, { to: 'debug' })
    return result
  })

  on('turn.step', async function* ($, e, next) {
    if (stopped || e.index === 0 || e.agentId !== undefined) return yield* next(e)
    const account = await $.env.get('ANCHORLEG_ACCOUNT')
    const bin = await $.env.get('ANCHORLEG_BIN')
    const runId = await $.env.get('ANCHORLEG_RUN_ID')
    const stopAt = Number((await $.env.get('ANCHORLEG_STOP_AT')) ?? 'NaN')
    if (!account || !bin || !runId || !(stopAt > 0)) return yield* next(e)

    const { rateLimits } = await $.session.usage()
    const used = maxUsed(rateLimits)
    if (used < stopAt) return yield* next(e)

    const report = {
      account,
      readings: toReadings(rateLimits),
      run_id: Number(runId),
      stop_reason: `quota at ${Math.round(used * 100)}%`,
    }
    const run = await $.process.run([bin, 'report', '--json'], {
      stdin: JSON.stringify(report),
      timeoutMs: 10_000,
    })
    if (run.exitCode !== 0) {
      // anchorleg didn't get the stop; carry on rather than end a turn nobody will resume.
      $.ui.log(`anchorleg report failed: ${run.stderr.trim()}`, { to: 'debug' })
      return yield* next(e)
    }
    stopped = true
    // Answer the step ourselves: no request, no tool calls, so the turn ends here.
    return {
      turnId: e.turnId,
      index: e.index,
      answer: `[anchorleg] Pausing here: this account's quota is at ${Math.round(used * 100)}%. anchorleg will continue the task on another account.`,
      toolUses: [],
      stopReason: null,
      usage: null,
    }
  })

  on('tool.check', async ($, e, next) => {
    const verdict = await next(e)
    // Only the CLI's own question about a real call; a plugin's query (no id) never asks anyone.
    if (verdict.decision !== 'ask' || e.tool_use_id === undefined) return verdict
    const bin = await $.env.get('ANCHORLEG_BIN')
    const runId = await $.env.get('ANCHORLEG_RUN_ID')
    if (!bin || !runId) return verdict

    let id: number | undefined
    for (;;) {
      const request = {
        run_id: Number(runId),
        tool: e.tool,
        input: e.input ?? null,
        reason: verdict.reason ?? null,
        ...(id === undefined ? {} : { id }),
      }
      let answer: Answer
      try {
        // anchorleg answers "pending" before this times out; then we ask again with the id.
        const run = await $.process.run([bin, 'permission', 'ask', '--json'], {
          stdin: JSON.stringify(request),
          timeoutMs: 590_000,
        })
        if (run.exitCode !== 0) throw new Error(run.stderr.trim())
        answer = JSON.parse(run.stdout) as Answer
      } catch (err) {
        $.ui.log(`anchorleg permission ask failed: ${String(err)}`, { to: 'debug' })
        return verdict
      }
      if (answer.decision === 'allow') return { decision: 'allow', reason: answer.reason }
      if (answer.decision === 'deny') {
        return { decision: 'deny', reason: `Not allowed: ${answer.reason ?? 'the user said no'} (asked through anchorleg).` }
      }
      if (answer.decision !== 'pending' || answer.id === undefined) return verdict
      id = answer.id
    }
  })
}
