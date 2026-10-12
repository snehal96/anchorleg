// anchorleg-mod: runs inside each Claude session that anchorleg starts (Phase 2, D8).
//
// - session.measure: sends the session's exact quota to `anchorleg report --json`.
// - turn.step: before each model request after the first, if any window is at or above
//   ANCHORLEG_STOP_AT, records a stop for anchorleg and ends the turn without making the request.
//   The previous step's tool results are already in the transcript, so the next account can
//   resume from a clean point. anchorleg then switches accounts.
// - turn.step also checks `anchorleg interrupt-check`: when the person sent a message with
//   "interrupt", the turn ends before the next request and anchorleg continues the session with
//   that message (Claude has no way to take a message mid-turn).
// - tool.check: when Claude would ask the person about a tool call (in `-p` that ask becomes a
//   refusal), asks anchorleg instead (`anchorleg permission ask`) and waits for the person's answer in
//   `anchorleg ui` or `anchorleg permission answer`. Nothing is allowed without that answer; if anchorleg
//   can't be reached the CLI's own verdict stands.
// - ask_user: `-p` runs drop Claude's own AskUserQuestion tool (seen on 2.1.296), so in an
//   anchorleg run the mod registers `ask_user` (the model calls it `mcp__anchorleg-mod__ask_user`)
//   with the same input. Its questions go to anchorleg (the same queue, as tool
//   `AskUserQuestion`) and the person's answers come back as the result. AskUserQuestion itself
//   is answered the same way where the CLI does offer it. Skipped questions are a refusal the
//   model reads.
//
// It never touches credentials (D8): anchorleg does the switch by relaunching the CLI.
// Outside anchorleg (no ANCHORLEG_ACCOUNT) it does nothing.

import type { EngineInterface, Register, SessionRateLimit } from 'claude-code'

type Reading = { window: string; used: number; resets_at?: number }

/** What `anchorleg permission ask --json` prints. */
type Answer = { decision?: string; id?: number; reason?: string; answers?: Record<string, string[]> }

/** A question as anchorleg stores it (crates/anchorleg-core/src/questions.rs). */
type Question = {
  id: string
  header?: string
  question: string
  options: { label: string; description?: string }[]
  multi_select: boolean
}

type ClaudeQuestion = {
  question: string
  header?: string
  options?: readonly { label: string; description?: string }[]
  multiSelect?: boolean
}

/** Claude's questions in anchorleg's shape; answers are keyed by the question text. */
export function toQuestions(questions: readonly ClaudeQuestion[]): Question[] {
  return questions.map((q) => ({
    id: q.question,
    ...(q.header ? { header: q.header } : {}),
    question: q.question,
    options: (q.options ?? []).map((o) => ({
      label: o.label,
      ...(o.description ? { description: o.description } : {}),
    })),
    multi_select: q.multiSelect === true,
  }))
}

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

type Ask = { tool: string; input: unknown; reason: string | null }

/**
 * Ask the person through `anchorleg permission ask --json` and wait for the answer: anchorleg
 * says "pending" before the process times out, and we ask again with the id. `undefined` when
 * this isn't an anchorleg run or anchorleg can't be reached.
 */
async function askAnchorleg($: EngineInterface, ask: Ask): Promise<Answer | undefined> {
  const bin = await $.env.get('ANCHORLEG_BIN')
  const runId = await $.env.get('ANCHORLEG_RUN_ID')
  if (!bin || !runId) return undefined
  let id: number | undefined
  for (;;) {
    const request = { run_id: Number(runId), ...ask, ...(id === undefined ? {} : { id }) }
    let answer: Answer
    try {
      const run = await $.process.run([bin, 'permission', 'ask', '--json'], {
        stdin: JSON.stringify(request),
        timeoutMs: 590_000,
      })
      if (run.exitCode !== 0) throw new Error(run.stderr.trim())
      answer = JSON.parse(run.stdout) as Answer
    } catch (err) {
      $.ui.log(`anchorleg permission ask failed: ${String(err)}`, { to: 'debug' })
      return undefined
    }
    if (answer.decision !== 'pending') return answer
    if (answer.id === undefined) return undefined
    id = answer.id
  }
}

/** The tool the mod adds for questions, and its name as the model calls it. */
const ASK_TOOL = 'ask_user'
export const ASK_TOOL_ID = 'mcp__anchorleg-mod__ask_user'

const ASK_SPEC = {
  name: ASK_TOOL,
  description:
    'Ask the person running this task one to four questions and wait for their answers. ' +
    'Use it when you need a decision only they can make (a choice between approaches, a ' +
    'missing requirement); otherwise pick a sensible default and say so. Give 2-4 options per ' +
    'question when you can; they may also answer in their own words.',
  isDeferred: false,
  inputSchema: {
    type: 'object',
    properties: {
      questions: {
        type: 'array',
        minItems: 1,
        maxItems: 4,
        items: {
          type: 'object',
          properties: {
            question: { type: 'string', description: 'The full question, ending with a question mark.' },
            header: { type: 'string', description: 'A short label, up to 12 characters.' },
            options: {
              type: 'array',
              items: {
                type: 'object',
                properties: { label: { type: 'string' }, description: { type: 'string' } },
                required: ['label'],
              },
            },
            multiSelect: { type: 'boolean', description: 'Several options may be picked.' },
          },
          required: ['question'],
        },
      },
    },
    required: ['questions'],
  },
}

/** Ask Claude's questions through anchorleg. `undefined`: anchorleg can't be asked. */
async function askQuestions(
  $: EngineInterface,
  claude: readonly ClaudeQuestion[],
): Promise<{ answers: Record<string, string> } | { deny: string } | undefined> {
  const questions = toQuestions(claude)
  const answer = await askAnchorleg($, { tool: 'AskUserQuestion', input: { questions }, reason: null })
  if (answer === undefined) return undefined
  if (answer.decision !== 'allow' || !answer.answers) {
    return { deny: `The user didn't answer: ${answer.reason ?? 'skipped'} (asked through anchorleg). Decide yourself or ask in your reply.` }
  }
  const answers: Record<string, string> = {}
  for (const q of questions) {
    const a = answer.answers[q.id]
    if (a && a.length > 0) answers[q.id] = a.join(', ')
  }
  return { answers }
}

export const register: Register = (on) => {
  let stopped = false

  on('session.start', async ($, e, next) => {
    if (await $.env.get('ANCHORLEG_RUN_ID')) await $.tool.register(ASK_SPEC)
    return next(e)
  })

  on('tool.call', { tool: ASK_TOOL_ID }, async ($, e) => {
    const input = e as unknown as { questions?: ClaudeQuestion[] }
    const out = await askQuestions($, input.questions ?? [])
    if (out === undefined) return { deny: 'anchorleg could not be reached to ask the question; decide yourself or ask in your reply.' }
    if ('deny' in out) return out
    const lines = Object.entries(out.answers).map(([q, a]) => `${q} → ${a}`)
    return { result: lines.length > 0 ? `The user answered:\n${lines.join('\n')}` : 'The user gave no answer.' }
  })

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
    if (bin && runId) {
      const check = await $.process.run([bin, 'interrupt-check', runId], { timeoutMs: 10_000 })
      if (check.exitCode === 0) {
        stopped = true
        return {
          turnId: e.turnId,
          index: e.index,
          answer: '[anchorleg] Stopping here: the user sent a new message.',
          toolUses: [],
          stopReason: null,
          usage: null,
        }
      }
    }
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
    // A question isn't a call to approve: let it through to the tool.call hook below, which
    // asks the person through anchorleg.
    if ((e.tool === 'AskUserQuestion' || e.tool === ASK_TOOL_ID) && (await $.env.get('ANCHORLEG_RUN_ID'))) {
      return { decision: 'allow' }
    }
    const verdict = await next(e)
    // Only the CLI's own question about a real call; a plugin's query (no id) never asks anyone.
    if (verdict.decision !== 'ask' || e.tool_use_id === undefined) return verdict
    const answer = await askAnchorleg($, { tool: e.tool, input: e.input ?? null, reason: verdict.reason ?? null })
    if (answer?.decision === 'allow') return { decision: 'allow', reason: answer.reason }
    if (answer?.decision === 'deny') {
      return { decision: 'deny', reason: `Not allowed: ${answer.reason ?? 'the user said no'} (asked through anchorleg).` }
    }
    return verdict
  })

  on('tool.call', { tool: 'AskUserQuestion' }, async ($, e, next) => {
    const out = await askQuestions($, e.questions)
    if (out === undefined) return next(e)
    if ('deny' in out) return out
    return { result: { questions: e.questions, answers: out.answers } }
  }).catch(($, e, next) => (next.called ? { deny: 'anchorleg: asking the question failed.' } : next(e)))
}
