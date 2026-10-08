import { expect, mock, test } from 'claude-code/testing'
import type { TestBody } from 'claude-code/testing'
import type { On, SessionRateLimit } from 'claude-code'
import { maxUsed, toReadings } from './register'

const ENV = {
  ANCHORLEG_ACCOUNT: 'claude-sm',
  ANCHORLEG_BIN: '/bin/anchorleg',
  ANCHORLEG_RUN_ID: '7',
  ANCHORLEG_STOP_AT: '0.9',
}

const limits = (fiveHour: number): SessionRateLimit[] => [
  { kind: 'five_hour', percentUsed: fiveHour, resetsAt: '2026-10-08T13:40:00.000Z' },
  { kind: 'seven_day', percentUsed: 40 },
]

/** Answers the engine beneath the plugin; records every `anchorleg` call the mod makes. */
function engine(on: On, rateLimits: SessionRateLimit[], exitCode = 0) {
  const calls: { argv: readonly string[]; stdin?: string }[] = []
  let requests = 0
  on('process.run', (_$, e) => {
    calls.push({ argv: e.argv, stdin: e.init?.stdin })
    return {
      value: { exitCode, stdout: '', stderr: exitCode ? 'boom' : '', isStdoutTruncated: false, isStderrTruncated: false },
    }
  })
  on('session.usage', () => ({
    value: { startedAt: 0, context: { window: 1_000_000, tokens: 1000, percent: 0 }, rateLimits },
  }))
  on('session.measure', () => ({ changed: ['rateLimits'] }))
  on('turn.step', async function* (_$, e) {
    requests++
    return {
      turnId: e.turnId,
      index: e.index,
      answer: 'model answered',
      toolUses: [],
      stopReason: 'end_turn',
      usage: null,
    }
  })
  return { calls, requests: () => requests }
}

const step = (index: number) => ({ turnId: 't1', index, model: 'claude-haiku-5-5', messageCount: 3 })

type Engine = Parameters<TestBody>[0]

/** Runs one model step through the plugin and returns the step's result. */
async function runStep($: Engine, index: number, agentId?: string) {
  const stream = $.turn.step(agentId ? { ...step(index), agentId } : step(index))
  for (;;) {
    const r = await stream.next()
    if (r.done) return r.value
  }
}

test('converts percent and ISO time to anchorleg readings', () => {
  expect(toReadings(limits(91))).toEqual([
    { window: 'five_hour', used: 0.91, resets_at: 1791466800 },
    { window: 'seven_day', used: 0.4 },
  ])
  expect(maxUsed(limits(91))).toBe(0.91)
  expect(maxUsed([])).toBe(0)
})

test('session.measure reports quota to anchorleg', async ($, on) => {
  mock.env(on, ENV)
  const { calls } = engine(on, limits(30))
  await $.session.measure({ context: { window: 1_000_000 }, rateLimits: limits(30), changed: ['rateLimits'] })
  expect(calls.length).toBe(1)
  expect(calls[0].argv).toEqual(['/bin/anchorleg', 'report', '--json'])
  expect(JSON.parse(calls[0].stdin ?? '')).toEqual({
    account: 'claude-sm',
    readings: toReadings(limits(30)),
  })
})

test('below the threshold the step goes to the model', async ($, on) => {
  mock.env(on, ENV)
  const { calls, requests } = engine(on, limits(50))
  const result = await runStep($, 1)
  expect(result.answer).toBe('model answered')
  expect(requests()).toBe(1)
  expect(calls.length).toBe(0)
})

test('at the threshold it records a stop and ends the turn without a request', async ($, on) => {
  mock.env(on, ENV)
  const { calls, requests } = engine(on, limits(92))
  const result = await runStep($, 2)
  expect(requests()).toBe(0)
  expect(result.toolUses.length).toBe(0)
  expect(result.answer).toContain('92%')
  const sent = JSON.parse(calls[0].stdin ?? '')
  expect(sent.run_id).toBe(7)
  expect(sent.stop_reason).toBe('quota at 92%')
  // Only once per session: the next step goes through.
  await runStep($, 3)
  expect(requests()).toBe(1)
})

test('the first step and subagent steps are never stopped', async ($, on) => {
  mock.env(on, ENV)
  const { requests } = engine(on, limits(99))
  await runStep($, 0)
  await runStep($, 4, 'a1')
  expect(requests()).toBe(2)
})

test('if anchorleg does not get the stop, the turn carries on', async ($, on) => {
  mock.env(on, ENV)
  const { requests } = engine(on, limits(99), 1)
  await runStep($, 1)
  expect(requests()).toBe(1)
})

test('outside anchorleg it does nothing', async ($, on) => {
  mock.env(on, {})
  const { calls, requests } = engine(on, limits(99))
  await $.session.measure({ context: { window: 1_000_000 }, rateLimits: limits(99), changed: ['rateLimits'] })
  await runStep($, 1)
  expect(calls.length).toBe(0)
  expect(requests()).toBe(1)
})

/** The engine's permission verdict for every call, and anchorleg's answers in order. */
function asking(on: On, answers: string[], exitCode = 0) {
  const calls: { argv: readonly string[]; stdin?: string }[] = []
  on('process.run', (_$, e) => {
    calls.push({ argv: e.argv, stdin: e.init?.stdin })
    return {
      value: { exitCode, stdout: answers.shift() ?? '', stderr: '', isStdoutTruncated: false, isStderrTruncated: false },
    }
  })
  on('tool.check', (_$, e) =>
    e.tool === 'Read' ? { decision: 'allow' } : { decision: 'ask', reason: 'not granted yet' },
  )
  return calls
}

const write = { tool: 'Write', input: { file_path: '/r/a.txt', content: 'x' }, tool_use_id: 'tu1' }

test('a call Claude would ask about waits for the answer in anchorleg', async ($, on) => {
  mock.env(on, ENV)
  const calls = asking(on, [
    JSON.stringify({ decision: 'pending', id: 3, reason: 'still waiting' }),
    JSON.stringify({ decision: 'allow', id: 3, reason: 'the user allowed it' }),
  ])
  const verdict = await $.tool.check(write)
  expect(verdict.decision).toBe('allow')
  expect(calls.length).toBe(2)
  expect(calls[0].argv).toEqual(['/bin/anchorleg', 'permission', 'ask', '--json'])
  expect(JSON.parse(calls[0].stdin ?? '')).toEqual({
    run_id: 7,
    tool: 'Write',
    input: write.input,
    reason: 'not granted yet',
  })
  expect(JSON.parse(calls[1].stdin ?? '').id).toBe(3)
})

test('a no from the user is a refusal the model reads', async ($, on) => {
  mock.env(on, ENV)
  asking(on, [JSON.stringify({ decision: 'deny', id: 4, reason: 'the user said no' })])
  const verdict = await $.tool.check(write)
  expect(verdict.decision).toBe('deny')
  expect(verdict.reason).toContain('the user said no')
})

test('allowed calls, queries and runs outside anchorleg never ask', async ($, on) => {
  mock.env(on, ENV)
  const calls = asking(on, [])
  expect((await $.tool.check({ ...write, tool: 'Read' })).decision).toBe('allow')
  expect((await $.tool.check({ tool: 'Write', input: write.input })).decision).toBe('ask')
  expect(calls.length).toBe(0)
})

test('if anchorleg cannot answer, the CLI verdict stands', async ($, on) => {
  mock.env(on, ENV)
  asking(on, ['oops'], 1)
  expect((await $.tool.check(write)).decision).toBe('ask')
})
