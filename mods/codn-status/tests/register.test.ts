import type { On, SessionRateLimit, SessionUsage, TurnStepChunk, TurnStepResult, TurnStepServerToolUse, TurnUsage } from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'
import type { Engine, MockClock } from 'claude-code/testing'

const FILE = '/state/codn/status/1/1-1.toml'

const USAGE: SessionUsage = {
  startedAt: 0,
  context: { tokens: 124000, window: 200000, percent: 62 },
  rateLimits: [{ kind: 'five_hour', percentUsed: 34 }],
}

// What the plugin writes for USAGE. src/core/reported.rs of codn reads the
// same text in a test of its own: change both together.
const REPORT = [
  'model = "opus"',
  'options = ["high", "think"]',
  'tokens = 124000',
  'window = 200000',
  '',
  '[limits."5h"]',
  'used = 34',
  '',
].join('\n')

// What the plugin asks the session, beneath it: codn's variable, a model,
// which a test may change as it goes, the settings, the figures.
const ground = (
  on: On,
  variables: Record<string, string> = { CODN_STATUS: FILE },
  model: string | (() => string) = 'opus',
  settings: Record<string, unknown> = { effortLevel: 'high' },
): void => {
  mock.env(on, variables)
  clock = mock.clock(on)
  on('session.model', () => ({ value: typeof model === 'string' ? model : model() }))
  on('settings.read', () => ({ value: settings }))
  on('classic.Stop', () => ({}))
  on('session.usage', () => ({ value: USAGE }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('session.measure', ($, e) => ({ changed: e.changed }))
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))
  on('turn.complete', ($, e) => ({ text: e.answer }))
}

// The world beneath the plugin: that, tools that run, and a file system
// that remembers what was written.
const world = (
  on: On,
  variables: Record<string, string> = { CODN_STATUS: FILE },
  model: string | (() => string) = 'opus',
  isWritable = true,
  settings: Record<string, unknown> = { effortLevel: 'high' },
): string[] => {
  const written: string[] = []
  ground(on, variables, model, settings)
  on('tool.call', () => ({ result: 'ran' }))
  on('fs.write', ($, e) => {
    written.push(`${e.path}\n${e.text}`)
    if (!isWritable) throw new Error('read-only file system')

    return { value: undefined }
  })

  return written
}

// The clock of the test at hand, which lets what does not wait come to its
// end, as does a turn of the timer where a test has no clock.
let clock: MockClock | undefined
const settled = async (): Promise<void> => (clock === undefined ? idle() : clock.settle())

// An event, and then what the plugin writes of it: no event waits for
// that, so the test does.
const after = async <T>(event: Promise<T>): Promise<T> => {
  const came = await event
  await settled()

  return came
}

const start = ($: Engine, isInteractive = true) =>
  after($.session.start({ surface: isInteractive ? 'terminal' : null, isInteractive, cwd: '/work' }))

// The end of a turn, of the session or of an agent.
const turn = ($: Engine, agentId?: string) =>
  after($.turn.complete({ answer: 'done', durationMs: 1, isAborted: false, turnId: 't', reason: 'answer', agentId }))

// A call of a tool as the loop of an agent makes it, or with no agent the
// loop of the session.
const call = ($: Engine, agentId: string | undefined, input: Record<string, unknown>) =>
  after($.tool.call({ ...input, agentId } as never))

// What moved in a measurement that names the context alone.
const context = (tokens: number | undefined, window: number) => ({
  context: { tokens, window },
  rateLimits: [] as SessionRateLimit[],
  changed: ['context' as const],
})

// How a model answers a request: with so many tokens, or without saying
// how many, and in what time, before the first piece of the answer and
// from there to the last. What the API ran itself inside the answer is on
// the result.
type Answering = {
  tokens: number | null
  waits?: number
  takes: number
  serverToolUses?: TurnStepServerToolUse[]
}

// The timer of the program that runs the tests, and its clock. The types
// of a plugin name neither: a plugin has none.
declare const setTimeout: (run: () => void, ms: number) => unknown
declare const performance: { now: () => number }

// Lets what does not wait come to its end, where the test has no clock
// that does. The tests of the speed have none: the clock of the kit,
// moved while an answer streams, takes half a second that is real.
// They tell the time themselves and move it between two pieces.
function idle(): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, 0))
}

// How often the plugin asked the clock of the test at hand.
let asked = 0

// The world beneath the plugin with a model in it that answers as
// `answering` says when it is asked, and a clock that this model moves:
// no test waits for an answer. The clock says its time unless `isTelling`
// says no, and the session its figures unless `isKnown` does.
const answered = (
  on: On,
  answering: Answering,
  isTelling: () => boolean = () => true,
  isKnown: () => boolean = () => true,
  variables: Record<string, string> = { CODN_STATUS: FILE },
): string[] => {
  const written: string[] = []
  let now = 0
  clock = undefined
  asked = 0
  mock.env(on, variables)
  on('clock.now', () => {
    asked += 1
    if (!isTelling()) throw new Error('no clock')

    return { value: now }
  })
  on('session.model', () => ({ value: 'opus' }))
  on('settings.read', () => ({ value: { effortLevel: 'high' } }))
  on('session.usage', () => {
    if (!isKnown()) throw new Error('not yet')

    return { value: USAGE }
  })
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))
  on('turn.complete', ($, e) => ({ text: e.answer }))
  on('fs.write', ($, e) => {
    written.push(`${e.path}\n${e.text}`)

    return { value: undefined }
  })
  on('turn.step', async function* ($, e) {
    const { tokens, waits = 0, takes, serverToolUses } = answering
    const usage =
      tokens === null
        ? null
        : { input_tokens: 9, output_tokens: tokens, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, model: 'opus' }
    now += waits
    yield { kind: 'text', index: 0, text: 'done' }
    // The plugin asked the time and did not wait for it: it is told before
    // the clock moves.
    await idle()
    now += takes
    yield { kind: 'stop', stopReason: 'end_turn', usage }

    return { turnId: e.turnId, index: e.index, answer: 'done', toolUses: [], serverToolUses, stopReason: 'end_turn', usage }
  })

  return written
}

// A request of a turn, of the session or of an agent, read to its end.
// The answer does not wait for what the plugin writes of it, so the test
// does.
const step = async ($: Engine, agentId?: string): Promise<{ chunks: TurnStepChunk[]; result: TurnStepResult }> => {
  const stream = $.turn.step({ turnId: 't', index: 0, model: 'opus', messageCount: 1, agentId })
  const chunks: TurnStepChunk[] = []
  let read = await stream.next()
  while (!read.done) {
    chunks.push(read.value)
    read = await stream.next()
  }
  await idle()

  return { chunks, result: read.value }
}

// The report with a speed in it, where the plugin writes it: before the
// limits.
const fast = (speed: number): string => REPORT.replace('\n\n[limits', `\nspeed = ${speed}\n\n[limits`)

// The cost of an answer as the API reports it, of so many output tokens.
const usageOf = (tokens: number): TurnUsage => ({
  input_tokens: 9,
  output_tokens: tokens,
  cache_read_input_tokens: 0,
  cache_creation_input_tokens: 0,
  model: 'opus',
})

// How a model answers, piece by piece: a piece as the engine yields it, a
// time that passes before the next, or the stream breaking off.
type Script = (TurnStepChunk | { after: number } | { breaks: string })[]

// The world beneath the plugin with a model that answers as the script
// says, which the test may rewrite between requests. The result of a
// request says what its last stop said. The clock tells its time as
// `told` has it, which may take its time to say so.
const scripted = (
  on: On,
  script: Script,
  result: Partial<TurnStepResult> = {},
  told: (now: number) => number | Promise<number> = now => now,
): string[] => {
  const written: string[] = []
  let now = 0
  clock = undefined
  mock.env(on, { CODN_STATUS: FILE })
  on('clock.now', async () => ({ value: await told(now) }))
  on('session.model', () => ({ value: 'opus' }))
  on('settings.read', () => ({ value: { effortLevel: 'high' } }))
  on('session.usage', () => ({ value: USAGE }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))
  on('turn.complete', ($, e) => ({ text: e.answer }))
  on('fs.write', ($, e) => {
    written.push(`${e.path}\n${e.text}`)

    return { value: undefined }
  })
  on('turn.step', async function* ($, e) {
    let stopReason: TurnStepResult['stopReason'] = null
    let usage: TurnStepResult['usage'] = null
    for (const item of script) {
      if ('after' in item) {
        // As `answered` does: the plugin asked the time and did not wait
        // for it, so it is told before the clock moves.
        await idle()
        now += item.after
        continue
      }
      if ('breaks' in item) throw new Error(item.breaks)
      if (item.kind === 'stop') ({ stopReason, usage } = item)
      yield item
    }

    return { turnId: e.turnId, index: e.index, answer: '', toolUses: [], stopReason, usage, ...result }
  })

  return written
}

// A request read to its end, or as far as it goes: the pieces that came,
// and what ended the stream where it was not the result.
const readAll = async ($: Engine): Promise<{ chunks: TurnStepChunk[]; broke?: string }> => {
  const stream = $.turn.step({ turnId: 't', index: 0, model: 'opus', messageCount: 1 })
  const chunks: TurnStepChunk[] = []
  try {
    for await (const chunk of stream) chunks.push(chunk)
  } catch (error) {
    await idle()

    return { chunks, broke: String(error) }
  }
  await idle()

  return { chunks }
}

const text = (text: string): TurnStepChunk => ({ kind: 'text', index: 0, text })
const stop = (tokens: number | null): TurnStepChunk => ({
  kind: 'stop',
  stopReason: 'end_turn',
  usage: tokens === null ? null : usageOf(tokens),
})

// What stands behind the report of the session in what was written last.
const agents = (written: string[]): string => (written.at(-1) ?? '').slice(`${FILE}\n${REPORT}`.length)

describe('register', () => {
  test('the start of a session reports what it knows', async ($, on) => {
    const written = world(on)

    await start($)

    expect(written).toEqual([`${FILE}\n${REPORT}`])
  })

  test('a measurement is written as the report codn reads', async ($, on) => {
    const written = world(on)
    await start($)
    written.length = 0

    await after($.session.measure({
      context: { tokens: 308643.4, window: 1000000, percent: 31 },
      rateLimits: [
        { kind: 'five_hour', percentUsed: 34, resetsAt: '2026-10-06T19:36:00Z' },
        { kind: 'seven_day', percentUsed: 12.5, resetsAt: 'when it pleases' },
        { kind: 'spend_limit', percentUsed: Number.NaN },
      ],
      cost: { usd: 1.2 },
      changed: ['context'],
    }))

    expect(written).toEqual([
      [
        FILE,
        'model = "opus"',
        'options = ["high", "think"]',
        'tokens = 308643',
        'window = 1000000',
        '',
        '[limits."5h"]',
        'used = 34',
        'resets = 1791315360',
        '',
        '[limits."7d"]',
        'used = 12.5',
        '',
      ].join('\n'),
    ])
  })

  test('a session without figures yet reports its model', async ($, on) => {
    const written = world(on)
    await start($)
    written.length = 0

    await after($.session.measure(context(undefined, 200000)))

    expect(written).toEqual([`${FILE}\nmodel = "opus"\noptions = ["high", "think"]\nwindow = 200000\n`])
  })

  test('the options are those of the settings until a turn says otherwise', async ($, on) => {
    const settings = {
      effortLevel: 'low',
      alwaysThinkingEnabled: false,
      modelSettings: { opus: { effortLevel: 'medium' }, other: { effortLevel: 'max' } },
    }
    const written = world(on, { CODN_STATUS: FILE }, 'opus', true, settings)
    const options = () => written.splice(0).map(text => text.split('\n')[2])

    // What is set for the model wins over what is set for all of them,
    // and a session that does not think says nothing of it.
    await start($)
    expect(options()).toEqual(['options = ["medium"]'])

    // A subagent stops with its own effort, which is not the session's.
    await after($.classic.Stop({ stop_hook_active: false, agent_id: 'agent-1', effort: { level: 'low' } }))
    expect(options()).toEqual([])

    // The session was started with another effort than the settings name,
    // and the reports that follow keep it.
    await after($.classic.Stop({ stop_hook_active: false, effort: { level: 'xhigh' } }))
    expect(options()).toEqual(['options = ["xhigh"]'])
    await after($.session.measure(context(99, 100)))
    expect(options()).toEqual(['options = ["xhigh"]'])
  })

  test('a session with nothing set runs with thought alone', async ($, on) => {
    const written = world(on, { CODN_STATUS: FILE }, 'opus', true, {})

    await start($)

    expect(written[0]?.split('\n')[2]).toBe('options = ["think"]')
  })

  test('what TOML or codn would not take is left out or made to fit', async ($, on) => {
    const written = world(on, { CODN_STATUS: FILE }, 'o\u0007p\u007fu\ud800s "5"')
    await start($)
    written.length = 0

    await after($.session.measure({
      context: { tokens: 1e20, window: 1e300 },
      rateLimits: [
        { kind: 'five_hour', percentUsed: 1e21, resetsAt: '2026-10-06T19:36:00.900Z' },
        { kind: 'five_hour', percentUsed: 2 },
        { kind: '__proto__', percentUsed: 3 },
        { kind: 'constructor', percentUsed: 4 },
        { kind: 'a\nb', percentUsed: 5 },
        { kind: 'below', percentUsed: -1 },
        { kind: 'zero', percentUsed: 0 },
      ],
      changed: ['context'],
    }))

    // The four limits codn shows, and not the fifth.
    expect(written).toEqual([
      [
        FILE,
        'model = "opus \\"5\\""',
        'options = ["high", "think"]',
        '',
        '[limits."5h"]',
        'used = 999',
        'resets = 1791315361',
        '',
        '[limits."__proto__"]',
        'used = 3',
        '',
        '[limits."constructor"]',
        'used = 4',
        '',
        '[limits."ab"]',
        'used = 5',
        '',
      ].join('\n'),
    ])
  })

  test('a name is held against what codn reads of one', async ($, on) => {
    let model = ''
    const settings = { effortLevel: '' }
    const written = world(on, { CODN_STATUS: FILE }, () => model, true, settings)
    await start($)
    const said = async (name: string, effort: string): Promise<string[]> => {
      written.length = 0
      model = name
      settings.effortLevel = effort
      await turn($)

      return (written[0] ?? '').split('\n').slice(1, 3)
    }

    // No longer than codn shows, counted in whole characters.
    expect(await said('m'.repeat(60), 'e'.repeat(60))).toEqual([
      `model = "${'m'.repeat(48)}"`,
      `options = ["${'e'.repeat(48)}", "think"]`,
    ])
    expect(await said(`${'m'.repeat(47)}\u{1f600}s`, 'high')).toEqual([
      `model = "${'m'.repeat(47)}\u{1f600}"`,
      'options = ["high", "think"]',
    ])
    // codn counts every dot, brace and bracket of a report, and reads none
    // that has too many: a name with more of them than a name may have is
    // none, so that what else there is to say is still read.
    expect(await said('a.b.c.d.e{f[g.h.i', 'high')).toEqual(['model = "a.b.c.d.e{f[g.h.i"', 'options = ["high", "think"]'])
    expect(await said('a.b.c.d.e{f[g.h.i.j', 'high')).toEqual(['options = ["high", "think"]', 'tokens = 124000'])
    expect(await said('opus', '[[[[[[[[[')).toEqual(['model = "opus"', 'options = ["think"]'])
    // Nor is a name of nothing that is seen one.
    expect(await said(' \u0007 ', 'high')).toEqual(['options = ["high", "think"]', 'tokens = 124000'])
    // Three quotes in a row begin a text of several lines, and codn reads
    // no agents behind a report that could hold one: a quote of that kind
    // is written as its number.
    expect(await said("o'''pus", "hi'gh")).toEqual([
      'model = "o\\u0027\\u0027\\u0027pus"',
      'options = ["hi\\u0027gh", "think"]',
    ])
  })

  test('no more limits are written than codn shows, under names it reads', async ($, on) => {
    const written = world(on)
    await start($)
    written.length = 0

    await $.session.measure({
      context: { window: 200000 },
      rateLimits: [
        { kind: 'l'.repeat(60), percentUsed: 1 },
        { kind: 'a.b.c.d.e.f.g.h.i.j', percentUsed: 2 },
        { kind: '', percentUsed: 3 },
        { kind: 7 as never, percentUsed: 3 },
        { kind: "it's", percentUsed: 4 },
        { kind: 'zero', percentUsed: 0 },
        { kind: 'five_hour', percentUsed: 5 },
        { kind: 'seven_day', percentUsed: 6 },
      ],
      changed: ['rateLimits'],
    })
    await settled()

    expect(written).toEqual([
      [
        FILE,
        'model = "opus"',
        'options = ["high", "think"]',
        'window = 200000',
        '',
        `[limits."${'l'.repeat(48)}"]`,
        'used = 1',
        '',
        '[limits."it\\u0027s"]',
        'used = 4',
        '',
        '[limits."zero"]',
        'used = 0',
        '',
        '[limits."5h"]',
        'used = 5',
        '',
      ].join('\n'),
    ])
  })

  test('a figure that is no number, or one without end, is left out', async ($, on) => {
    const written = world(on)
    await start($)
    written.length = 0

    await $.session.measure({
      context: { tokens: '5' as never, window: Number.POSITIVE_INFINITY },
      rateLimits: [
        { kind: 'five_hour', percentUsed: Number.POSITIVE_INFINITY },
        { kind: 'seven_day', percentUsed: '7' as never },
      ],
      changed: ['context'],
    })
    await settled()

    expect(written).toEqual([`${FILE}\nmodel = "opus"\noptions = ["high", "think"]\n`])
  })

  test('the end of a turn reports, that of a subagent does not', async ($, on) => {
    // The model was changed in the turn, which moves no figure.
    let model = 'opus'
    const written = world(on, { CODN_STATUS: FILE }, () => model)
    await start($)
    written.length = 0
    model = 'sonnet'

    await turn($, 'agent-1')
    expect(written).toEqual([])

    await turn($)
    expect(written).toEqual([`${FILE}\n${REPORT.replace('opus', 'sonnet')}`])
  })

  test('after /clear the limits stay and the tokens go', async ($, on) => {
    const written = world(on)
    await start($)
    written.length = 0

    await after($.session.end({ reason: 'other', sessionId: 's', resume: { id: 's' } }))
    expect(written).toEqual([])

    await after($.session.end({ reason: 'clear', sessionId: 's', resume: { id: 's' } }))
    expect(written).toEqual([
      `${FILE}\nmodel = "opus"\noptions = ["high", "think"]\nwindow = 200000\n\n[limits."5h"]\nused = 34\n`,
    ])
  })

  test('a session nobody sits at leaves the report of the one that started it', async ($, on) => {
    const written = world(on)

    await start($, false)
    await after($.session.measure(context(undefined, 200000)))
    await turn($)
    await after($.session.end({ reason: 'clear', sessionId: 's', resume: { id: 's' } }))

    expect(written).toEqual([])
  })

  test('outside codn nothing is written', async ($, on) => {
    const written = world(on, {})

    await start($)
    await after($.session.measure(context(undefined, 200000)))

    expect(written).toEqual([])
  })

  test('a report that cannot be written stops nothing', async ($, on) => {
    const tried = world(on, { CODN_STATUS: FILE }, 'opus', false)
    await start($)

    const measured = await after($.session.measure(context(undefined, 200000)))

    expect(measured).toEqual({ changed: ['context'] })
    expect(tried.length).toBe(2)
  })

  test('what an agent is doing is written behind the report, by its id', async ($, on) => {
    const written = world(on)
    await start($)
    written.length = 0

    // The words the model writes of a command. src/core/reported.rs of
    // codn reads this text in a test of its own: change both together.
    const ran = await call($, 'a89b5d4516757b675', { tool: 'Bash', command: 'date +%T', description: 'Show current time' })
    expect(ran).toMatchObject({ result: 'ran' })
    expect(written).toEqual([`${FILE}\n${REPORT}\n[agents]\n"a89b5d4516757b675" = "Show current time"\n`])

    // The same again says nothing new, nor does a call of the session
    // itself, nor a tool whose call says nothing a person would read.
    await call($, 'a89b5d4516757b675', { tool: 'Bash', command: 'date +%T', description: 'Show current time' })
    await call($, undefined, { tool: 'Bash', command: 'ls', description: 'List files' })
    await call($, 'a89b5d4516757b675', { tool: 'TodoWrite', todos: [] })
    expect(written.length).toBe(1)

    // The one that said so last stands last.
    await call($, 'aad878bc0972d0d46', { tool: 'Read', file_path: '/work/src/ui/sidebar.rs' })
    await call($, 'a89b5d4516757b675', { tool: 'Grep', pattern: 'OpenBelow' })
    expect(agents(written)).toBe('\n[agents]\n"aad878bc0972d0d46" = "Read sidebar.rs"\n"a89b5d4516757b675" = "Grep OpenBelow"\n')
  })

  test('an agent that has ended is taken out, and the report stays', async ($, on) => {
    const written = world(on)
    await start($)
    await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
    await call($, 'a2', { tool: 'Read', file_path: 'b.rs' })
    written.length = 0

    await turn($, 'a1')
    expect(written).toEqual([`${FILE}\n${REPORT}\n[agents]\n"a2" = "Read b.rs"\n`])
    await turn($, 'a2')
    expect(written.at(-1)).toBe(`${FILE}\n${REPORT}`)

    // What the session reports later has the agents behind it too.
    await call($, 'a3', { tool: 'Read', file_path: 'c.rs' })
    written.length = 0
    await after($.session.measure(context(99, 100)))
    expect(written).toEqual([
      `${FILE}\nmodel = "opus"\noptions = ["high", "think"]\ntokens = 99\nwindow = 100\n\n[agents]\n"a3" = "Read c.rs"\n`,
    ])
  })

  test('a call is put into a few words of one line', async ($, on) => {
    const written = world(on)
    await start($)
    const doing = async (input: Record<string, unknown>, agentId = 'a1') => {
      written.length = 0
      await call($, agentId, input)

      // The one that said so last stands last.
      return agents(written).trim().split('\n').at(-1)
    }

    // What the model writes of a command, in one line and cut where codn
    // would cut it. Of a command it writes nothing of, the word alone:
    // what a command is made of can be a secret.
    expect(await doing({ tool: 'Bash', command: 'c', description: 'Run the\n  tests' })).toBe('"a1" = "Run the tests"')
    expect(await doing({ tool: 'Bash', command: 'c', description: 'x'.repeat(80) })).toBe(`"a1" = "${'x'.repeat(47)}\u2026"`)
    expect(await doing({ tool: 'Bash', command: 'curl -H "Authorization: Bearer abc" x', description: ' ' })).toBe('"a1" = "Bash"')
    expect(await doing({ tool: 'Bash', command: 'TOKEN=abc deploy' }, 'a9')).toBe('"a9" = "Bash"')
    expect(await doing({ tool: 'Write', file_path: '/work/docs/' })).toBe('"a1" = "Write docs"')
    expect(await doing({ tool: 'NotebookEdit', notebook_path: 'n/plot.ipynb', new_source: '' })).toBe('"a1" = "Edit plot.ipynb"')
    expect(await doing({ tool: 'Glob', pattern: 'src/**/*.rs' })).toBe('"a1" = "Glob src/**/*.rs"')
    expect(await doing({ tool: 'WebFetch', url: 'https://example.org/a/b?c', prompt: 'p' })).toBe('"a1" = "Fetch example.org"')
    // Not the name and the password an address can carry.
    expect(await doing({ tool: 'WebFetch', url: 'https://name:secret@example.net:8443/a@b', prompt: 'p' })).toBe('"a1" = "Fetch example.net:8443"')
    expect(await doing({ tool: 'WebFetch', url: 'name:secret@example.com/x', prompt: 'p' })).toBe('"a1" = "Fetch"')
    // Nor what stands where a machine would, and is not the name of one.
    expect(await doing({ tool: 'WebFetch', url: 'https://name:secret', prompt: 'p' }, 'a2')).toBe('"a2" = "Fetch"')
    expect(await doing({ tool: 'WebFetch', url: 'https://a b/c', prompt: 'p' }, 'a3')).toBe('"a3" = "Fetch"')
    expect(await doing({ tool: 'WebFetch', url: 'http://[::1]:8080/x', prompt: 'p' }, 'a4')).toBe('"a4" = "Fetch [::1]:8080"')
    expect(await doing({ tool: 'WebFetch', url: 'HTTPS://Docs.Example.org./', prompt: 'p' }, 'a5')).toBe('"a5" = "Fetch Docs.Example.org."')
    expect(await doing({ tool: 'WebSearch', query: 'osc 7501' })).toBe('"a1" = "Search osc 7501"')
    expect(await doing({ tool: 'Agent', description: 'Count the lines', prompt: 'p' })).toBe('"a1" = "Agent Count the lines"')
    expect(await doing({ tool: 'Skill', skill: 'handoff' })).toBe('"a1" = "Skill handoff"')
    expect(await doing({ tool: 'mcp__files__read_text', path: 'a' })).toBe('"a1" = "files: read_text"')
    // A tool that names nothing is the word alone.
    expect(await doing({ tool: 'Read' })).toBe('"a1" = "Read"')
    // What TOML would not take is made to fit, in the words and in the id,
    // which is made the id the terminal is told.
    expect(await doing({ tool: 'Bash', command: 'c', description: 'say "hi"\u0007 \\ there' })).toBe('"a1" = "say \\"hi\\" \\\\ there"')
    expect(await doing({ tool: 'Read', file_path: 'a.rs' }, 'team/one "x"')).toBe('"team-one--x-" = "Read a.rs"')
  })

  test('a machine is the names an address may have, joined by dots', async ($, on) => {
    const written = world(on)
    await start($)
    const fetched = async (url: string) => {
      written.length = 0
      // Under an id of its own: the same words again are not written again.
      await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
      await call($, 'a1', { tool: 'WebFetch', url, prompt: 'p' })

      return agents(written).trim().split('\n').at(-1)
    }

    for (const machine of ['a', 'xn--bcher-kva.example', '127.0.0.1:8080', 'a.b.c.d.e.f', 'example.org.:1', '[2001:db8::1]']) {
      expect(await fetched(`https://${machine}/x`)).toBe(`"a1" = "Fetch ${machine}"`)
    }
    for (const none of ['', '.', 'a..b', '.a', '-a.example', 'a-.example', 'a_b.example', 'a.example:123456', 'a.example:', ':80', '[::1', 'a!']) {
      expect(await fetched(`https://${none}/x`)).toBe('"a1" = "Fetch"')
    }
  })

  // What stands where a machine would, long and ending in a character no
  // machine has. A pattern that can cut it into names in many ways tries
  // them all, twice as many with every letter: 26 letters held the call
  // for more than half a second.
  const NO_MACHINE = `https://${'a'.repeat(40)}!/`

  test('an address of any length holds the call of a tool for no time', async ($, on) => {
    const written = world(on)
    await start($)

    const from = performance.now()
    await $.tool.call({ tool: 'WebFetch', url: NO_MACHINE, prompt: 'p', agentId: 'a1' } as never)
    const took = performance.now() - from
    await settled()

    expect(agents(written)).toBe('\n[agents]\n"a1" = "Fetch"\n')
    expect(took < 250).toBe(true)
  })

  test('an agent that begins anew under its id does not say what it did before', async ($, on) => {
    const written = world(on)
    on('classic.SubagentStart', () => ({}))
    await start($)
    await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
    await call($, 'a2', { tool: 'Read', file_path: 'b.rs' })
    written.length = 0

    await $.classic.SubagentStart({ agent_id: 'a1', agent_type: 'general-purpose' })
    await settled()
    expect(written).toEqual([`${FILE}\n${REPORT}\n[agents]\n"a2" = "Read b.rs"\n`])

    // One nothing was written of begins without a write.
    written.length = 0
    await $.classic.SubagentStart({ agent_id: 'a7', agent_type: 'general-purpose' })
    await settled()
    expect(written).toEqual([])
  })

  test('no more agents are written than codn shows, the latest of them', async ($, on) => {
    const written = world(on)
    await start($)

    for (let n = 0; n < 40; n++) await call($, `a${n}`, { tool: 'Read', file_path: `${n}.rs` })

    const lines = agents(written).trim().split('\n').slice(1)
    expect(lines.length).toBe(32)
    expect(lines[0]).toBe('"a8" = "Read 8.rs"')
    expect(lines.at(-1)).toBe('"a39" = "Read 39.rs"')
  })

  test('an agent of a session nobody sits at, or outside codn, writes nothing', async ($, on) => {
    const written = world(on)
    await start($, false)
    await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
    await turn($, 'a1')
    expect(written).toEqual([])
  })

  test('outside codn an agent writes nothing', async ($, on) => {
    const written = world(on, {})
    await start($)
    await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
    expect(written).toEqual([])
  })

  test('what an agent is doing that cannot be written stops no tool', async ($, on) => {
    const tried = world(on, { CODN_STATUS: FILE }, 'opus', false)
    await start($)

    const ran = await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })

    expect(ran).toMatchObject({ result: 'ran' })
    expect(tried.length).toBe(2)
  })

  // A write that waits for the disk until it is let go, for the first
  // text that has `mark` in it, and then fails if it is to. Every text
  // that was written is kept.
  const held = (on: On, mark: string, fails = false) => {
    let begun = () => {}
    let release = () => {}
    const begins = new Promise<void>(resolve => (begun = resolve))
    const waits = new Promise<void>(resolve => (release = resolve))
    const state = { text: '', tried: 0, isHeld: false, begins, release }
    on('fs.write', async ($, e) => {
      state.tried += 1
      if (!state.isHeld && e.text.includes(mark)) {
        state.isHeld = true
        begun()
        await waits
        if (fails) throw new Error('the disk is full')
      }
      state.text = e.text

      return { value: undefined }
    })

    return state
  }

  test('a write that is out holds no tool of an agent', async ($, on) => {
    ground(on)
    const disk = held(on, 'Old')
    let hasRun = false
    on('tool.call', () => {
      hasRun = true

      return { result: 'ran' }
    })
    await start($)

    const calling = call($, 'a1', { tool: 'Bash', command: 'true', description: 'Old' })
    await disk.begins
    const ranMeanwhile = hasRun
    disk.release()
    await calling

    expect(ranMeanwhile).toBe(true)
  })

  test('an older write of an agent does not land on a newer report', async ($, on) => {
    ground(on)
    on('tool.call', () => ({ result: 'ran' }))
    const disk = held(on, 'Old')
    await start($)

    const calling = call($, 'a1', { tool: 'Bash', command: 'true', description: 'Old' })
    await disk.begins
    await $.session.measure({ context: { tokens: 99, window: 100 }, rateLimits: [], changed: ['context'] })
    disk.release()
    await calling
    await settled()

    expect(disk.text.includes('tokens = 99')).toBe(true)
    expect(disk.text.includes('"a1" = "Old"')).toBe(true)
  })

  test('an older report does not land on what an agent did since', async ($, on) => {
    ground(on)
    on('tool.call', () => ({ result: 'ran' }))
    const disk = held(on, 'tokens = 99')
    await start($)
    await call($, 'a1', { tool: 'Bash', command: 'true', description: 'Old' })

    const measuring = $.session.measure({ context: { tokens: 99, window: 100 }, rateLimits: [], changed: ['context'] })
    await disk.begins
    await call($, 'a1', { tool: 'Bash', command: 'true', description: 'New' })
    disk.release()
    await measuring
    await settled()

    expect(disk.text.includes('"a1" = "New"')).toBe(true)
    expect(disk.text.includes('tokens = 99')).toBe(true)
  })

  test('a write that fails does not lose what changed while it was out', async ($, on) => {
    ground(on)
    on('tool.call', () => ({ result: 'ran' }))
    const disk = held(on, 'Old', true)
    await start($)

    const calling = call($, 'a1', { tool: 'Bash', command: 'true', description: 'Old' })
    await disk.begins
    await call($, 'a1', { tool: 'Bash', command: 'true', description: 'New' })
    disk.release()
    await calling
    await settled()

    expect(disk.text.includes('"a1" = "New"')).toBe(true)
    // The start, the write that failed, and the one for what changed:
    // a write that fails is not tried again for itself.
    expect(disk.tried).toBe(3)
  })

  test('the end of an agent does not wait for the write that takes it out', async ($, on) => {
    ground(on)
    on('tool.call', () => ({ result: 'ran' }))
    // A disk that answers until it is said not to, and then only when it
    // is let go.
    const disk = { isStuck: false, isAsked: false, release: () => {} }
    const waits = new Promise<void>(resolve => (disk.release = resolve))
    on('fs.write', async () => {
      if (disk.isStuck) {
        disk.isAsked = true
        await waits
      }

      return { value: undefined }
    })
    await start($)
    await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
    disk.isStuck = true

    let hasEnded = false
    const end = { answer: 'done', durationMs: 1, isAborted: false, turnId: 't', reason: 'answer', agentId: 'a1' } as const
    const ending = $.turn.complete(end).then(() => (hasEnded = true))
    await settled()
    const endedMeanwhile = hasEnded
    disk.release()
    await ending

    expect(disk.isAsked).toBe(true)
    expect(endedMeanwhile).toBe(true)
  })

  test('no event of the session waits for the write of its report', async ($, on) => {
    let model = 'opus'
    ground(on, { CODN_STATUS: FILE }, () => model)
    // A disk that answers only when it is let go.
    const disk = { asked: 0, release: () => {} }
    on('fs.write', async () => {
      disk.asked += 1
      await new Promise<void>(resolve => (disk.release = resolve))

      return { value: undefined }
    })
    // Whether an event came back while its write was out, and the how
    // manieth write that was.
    const cameBack = async (event: Promise<unknown>): Promise<[boolean, number]> => {
      let hasCome = false
      const coming = event.then(() => (hasCome = true))
      await settled()
      const seen: [boolean, number] = [hasCome, disk.asked]
      disk.release()
      await coming
      await settled()

      return seen
    }

    const starting = $.session.start({ surface: 'terminal', isInteractive: true, cwd: '/work' })
    expect(await cameBack(starting)).toEqual([true, 1])
    expect(await cameBack($.session.measure(context(99, 100)))).toEqual([true, 2])
    model = 'sonnet'
    const end = { answer: 'done', durationMs: 1, isAborted: false, turnId: 't', reason: 'answer' } as const
    expect(await cameBack($.turn.complete(end))).toEqual([true, 3])
    expect(await cameBack($.classic.Stop({ stop_hook_active: false, effort: { level: 'low' } }))).toEqual([true, 4])
    expect(await cameBack($.session.end({ reason: 'clear', sessionId: 's', resume: { id: 's' } }))).toEqual([true, 5])
  })

  test('a report that says what was written last is not written again', async ($, on) => {
    let model = 'opus'
    const written = world(on, { CODN_STATUS: FILE }, () => model)
    await start($)
    written.length = 0

    // The end of a turn is three reports, and mostly none says anything
    // the one before did not.
    await turn($)
    await after($.classic.Stop({ stop_hook_active: false, effort: { level: 'high' } }))
    await after($.session.measure({ context: USAGE.context, rateLimits: USAGE.rateLimits, changed: ['context'] }))
    expect(written).toEqual([])

    model = 'sonnet'
    await turn($)
    expect(written).toEqual([`${FILE}\n${REPORT.replace('opus', 'sonnet')}`])
  })

  test('a report that could not be written is written by the next one that says so', async ($, on) => {
    let model = 'opus'
    ground(on, { CODN_STATUS: FILE }, () => model)
    const disk = { isFull: false, tried: [] as string[] }
    on('fs.write', ($, e) => {
      disk.tried.push(e.text.split('\n')[0] ?? '')
      if (disk.isFull) throw new Error('the disk is full')

      return { value: undefined }
    })
    // A turn ends with this model, on a disk that is full or not.
    const ends = async (name: string, isFull: boolean) => {
      model = name
      disk.isFull = isFull
      await turn($)
    }
    await start($)

    // What the file holds after a write that failed is not known: the
    // text written before it is written again, and so is the text that
    // failed, though each is what was tried last or before.
    await ends('sonnet', true)
    await ends('opus', false)
    await ends('sonnet', true)
    await ends('sonnet', false)
    await ends('sonnet', false)

    expect(disk.tried).toEqual(['opus', 'sonnet', 'opus', 'sonnet', 'sonnet'].map(name => `model = "${name}"`))
  })

  test('a report that is made late is not put over one asked for after it', async ($, on) => {
    mock.env(on, { CODN_STATUS: FILE })
    clock = mock.clock(on)
    const written: string[] = []
    // The session is slow to say its figures the second time it is asked.
    let tell: (usage: SessionUsage) => void = () => {}
    const late = new Promise<SessionUsage>(resolve => (tell = resolve))
    let asks = 0
    on('session.model', () => ({ value: 'opus' }))
    on('settings.read', () => ({ value: { effortLevel: 'high' } }))
    on('session.usage', async () => ({ value: (asks += 1) === 2 ? await late : USAGE }))
    on('session.start', ($, e) => ({ cwd: e.cwd }))
    on('session.measure', ($, e) => ({ changed: e.changed }))
    on('turn.complete', ($, e) => ({ text: e.answer }))
    on('fs.write', ($, e) => {
      written.push(`${e.path}\n${e.text}`)

      return { value: undefined }
    })
    await start($)

    // A turn ends and asks, and a measurement that has its figures at
    // hand is reported before the session has answered.
    const ending = $.turn.complete({ answer: 'done', durationMs: 1, isAborted: false, turnId: 't', reason: 'answer' })
    await settled()
    await after($.session.measure(context(99, 100)))
    tell(USAGE)
    await ending
    await settled()

    expect(written.at(-1)).toBe(`${FILE}\nmodel = "opus"\noptions = ["high", "think"]\ntokens = 99\nwindow = 100\n`)
  })

  test('what agents do before a report could be made asks the session nothing', async ($, on) => {
    mock.env(on, { CODN_STATUS: FILE })
    clock = mock.clock(on)
    const written: string[] = []
    let asked = 0
    let isAnswering = false
    on('session.model', () => ({ value: 'opus' }))
    on('settings.read', () => ({ value: { effortLevel: 'high' } }))
    on('session.usage', () => {
      asked += 1
      if (!isAnswering) throw new Error('not yet')

      return { value: USAGE }
    })
    on('session.start', ($, e) => ({ cwd: e.cwd }))
    on('turn.complete', ($, e) => ({ text: e.answer }))
    on('tool.call', () => ({ result: 'ran' }))
    on('fs.write', ($, e) => {
      written.push(`${e.path}\n${e.text}`)

      return { value: undefined }
    })

    // The start could make no report: an agent that calls tools asks
    // the session no more for it, and nothing is written.
    await start($)
    expect([asked, written.length]).toEqual([1, 0])
    await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
    await call($, 'a1', { tool: 'Read', file_path: 'b.rs' })
    expect([asked, written.length]).toEqual([1, 0])

    // The next report of the session has what the agent is doing behind it.
    isAnswering = true
    await turn($)
    expect(written).toEqual([`${FILE}\n${REPORT}\n[agents]\n"a1" = "Read b.rs"\n`])
  })

  test('after /clear the agents of the conversation before are gone', async ($, on) => {
    const written = world(on)
    await start($)
    await call($, 'a1', { tool: 'Read', file_path: 'a.rs' })
    written.length = 0

    await after($.session.end({ reason: 'clear', sessionId: 's', resume: { id: 's' } }))
    expect(written).toEqual([
      `${FILE}\nmodel = "opus"\noptions = ["high", "think"]\nwindow = 200000\n\n[limits."5h"]\nused = 34\n`,
    ])

    // Another end of a session leaves them.
    await call($, 'a2', { tool: 'Read', file_path: 'b.rs' })
    written.length = 0
    await after($.session.end({ reason: 'other', sessionId: 's', resume: { id: 's' } }))
    await turn($)
    expect(agents(written)).toBe('\n[agents]\n"a2" = "Read b.rs"\n')
  })

  test('a call that names no agent by an id leaves no line', async ($, on) => {
    const written = world(on)
    await start($)
    written.length = 0

    await call($, '', { tool: 'Read', file_path: 'a.rs' })

    expect(written).toEqual([])
  })

  test('an answer of the model says the tokens a second it came at', async ($, on) => {
    // The wait for the first piece is none of it.
    const written = answered(on, { tokens: 150, waits: 5000, takes: 2000 })
    await start($)
    written.length = 0

    const { chunks, result } = await step($)

    expect(written).toEqual([`${FILE}\n${fast(75)}`])
    // What the model said is passed on as it came.
    expect(chunks).toEqual([
      { kind: 'text', index: 0, text: 'done' },
      { kind: 'stop', stopReason: 'end_turn', usage: expect.objectContaining({ output_tokens: 150 }) },
    ])
    expect(result).toEqual(expect.objectContaining({ answer: 'done', stopReason: 'end_turn' }))
  })

  test('the speed stays in a report that follows, also after /clear', async ($, on) => {
    const written = answered(on, { tokens: 150, takes: 2000 })
    await start($)
    await step($)
    written.length = 0

    await after($.session.end({ reason: 'clear', sessionId: 's', resume: { id: 's' } }))
    expect(written).toEqual([
      `${FILE}\nmodel = "opus"\noptions = ["high", "think"]\nwindow = 200000\nspeed = 75\n\n[limits."5h"]\nused = 34\n`,
    ])
  })

  test('an answer too short to say a speed leaves the one before', async ($, on) => {
    const answering: Answering = { tokens: 150, takes: 2000 }
    const written = answered(on, answering)
    const answer = (tokens: number | null, takes: number) => {
      Object.assign(answering, { tokens, takes })

      return step($)
    }
    await start($)
    await answer(150, 2000)
    written.length = 0

    // Less than a second, and an answer that does not say its tokens.
    await answer(900, 999)
    await answer(null, 2000)
    expect(written).toEqual([])

    // The same speed again is not written again.
    await answer(75, 1000)
    expect(written).toEqual([])

    await answer(80, 1000)
    expect(written).toEqual([`${FILE}\n${fast(80)}`])
  })

  test('the time the API ran a tool of its own is none of the answer', async ($, on) => {
    const written = answered(on, {
      tokens: 150,
      takes: 12000,
      serverToolUses: [{ id: 'u1', name: 'advisor', input: {}, startedAt: 1000, endedAt: 11000 }],
    })
    await start($)
    written.length = 0

    await step($)

    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('a tool of the API whose result did not arrive ran to the end of the answer', async ($, on) => {
    // The answer ended while the tool ran, as that of a turn that pauses
    // does: from the start of the tool on the model wrote nothing.
    const written = answered(on, {
      tokens: 150,
      takes: 12000,
      serverToolUses: [{ id: 'u1', name: 'advisor', input: {}, startedAt: 2000 }],
    })
    await start($)
    written.length = 0

    await step($)

    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('two tools of the API that ran at one time are taken off once', async ($, on) => {
    // From 2 s to 10 s one or the other ran: four seconds are the model's.
    const written = answered(on, {
      tokens: 150,
      takes: 12000,
      serverToolUses: [
        { id: 'u1', name: 'advisor', input: {}, startedAt: 2000, endedAt: 8000 },
        { id: 'u2', name: 'advisor', input: {}, startedAt: 4000, endedAt: 10000 },
        // One inside another, one that ended before it began, and one
        // whose times are no numbers take nothing more off.
        { id: 'u3', name: 'advisor', input: {}, startedAt: 5000, endedAt: 6000 },
        { id: 'u4', name: 'advisor', input: {}, startedAt: 11000, endedAt: 10500 },
        { id: 'u5', name: 'advisor', input: {}, startedAt: Number.NaN, endedAt: 11000 },
      ],
    })
    await start($)
    written.length = 0

    await step($)

    expect(written).toEqual([`${FILE}\n${fast(38)}`])
  })

  test('of a tool of the API only what lies in the answer is taken off', async ($, on) => {
    // The answer came from 5 s to 9 s. One tool ran from 0 s to 7 s, as
    // that of an answer before it in the same request does, and one
    // from 8.5 s to 20 s: a second and a half are the model's.
    const written = answered(on, {
      tokens: 150,
      waits: 5000,
      takes: 4000,
      serverToolUses: [
        { id: 'u1', name: 'advisor', input: {}, startedAt: 0, endedAt: 7000 },
        { id: 'u2', name: 'advisor', input: {}, startedAt: 8500, endedAt: 20000 },
      ],
    })
    await start($)
    written.length = 0

    await step($)

    expect(written).toEqual([`${FILE}\n${fast(100)}`])
  })

  test('an answer of no tokens says a speed of none', async ($, on) => {
    const answering: Answering = { tokens: 150, takes: 2000 }
    const written = answered(on, answering)
    await start($)
    await step($)
    written.length = 0

    Object.assign(answering, { tokens: 0 })
    await step($)

    expect(written).toEqual([`${FILE}\n${fast(0)}`])
  })

  test('a speed is a whole number, and one too high to be shown leaves the one before', async ($, on) => {
    const answering: Answering = { tokens: 150, takes: 2000 }
    const written = answered(on, answering)
    await start($)
    await step($)
    written.length = 0

    // More than codn shows, also where it would be cut to what it shows.
    Object.assign(answering, { tokens: 200_000, takes: 1000 })
    await step($)
    Object.assign(answering, { tokens: 499_998, takes: 5000 })
    await step($)
    expect(written).toEqual([])

    // The highest that is shown, and after it the same once more, which
    // is not written again.
    Object.assign(answering, { tokens: 499_997, takes: 5000 })
    await step($)
    Object.assign(answering, { tokens: 499_995, takes: 5000 })
    await step($)
    expect(written).toEqual([`${FILE}\n${fast(99999)}`])
  })

  test('an answer to an agent says no speed', async ($, on) => {
    const written = answered(on, { tokens: 150, takes: 2000 })
    await start($)
    written.length = 0

    const { chunks, result } = await step($, 'a1')

    expect(written).toEqual([])
    expect(chunks.map(chunk => chunk.kind)).toEqual(['text', 'stop'])
    expect(result.answer).toBe('done')
  })

  test('a speed before the first report waits for it', async ($, on) => {
    let isKnown = false
    const written = answered(
      on,
      { tokens: 150, takes: 2000 },
      () => true,
      () => isKnown,
    )

    await start($)
    await step($)
    expect(written).toEqual([])

    isKnown = true
    await turn($)
    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('a session nobody is at says no speed, and asks no clock for one', async ($, on) => {
    const written = answered(on, { tokens: 150, takes: 2000 })
    await start($, false)

    const { chunks, result } = await step($)

    expect([written, asked]).toEqual([[], 0])
    expect(chunks.map(chunk => chunk.kind)).toEqual(['text', 'stop'])
    expect(result.answer).toBe('done')
  })

  test('outside codn no clock is asked for a speed', async ($, on) => {
    const written = answered(
      on,
      { tokens: 150, takes: 2000 },
      () => true,
      () => true,
      {},
    )
    await start($)

    const { chunks, result } = await step($)

    expect([written, asked]).toEqual([[], 0])
    expect(chunks.map(chunk => chunk.kind)).toEqual(['text', 'stop'])
    expect(result.answer).toBe('done')
  })

  test('a clock that does not say leaves the answer as it is', async ($, on) => {
    const written = answered(on, { tokens: 150, takes: 2000 }, () => false)
    await start($)
    written.length = 0

    const { chunks, result } = await step($)

    expect(written).toEqual([])
    expect(chunks.map(chunk => chunk.kind)).toEqual(['text', 'stop'])
    expect(result.answer).toBe('done')
  })

  test('of a request answered anew, the last answer is the one', async ($, on) => {
    // A first answer of 50 tokens in a second, three seconds of nothing,
    // then the answer that counts: 150 tokens in two seconds.
    const written = scripted(on, [
      text('a'),
      { after: 1000 },
      stop(50),
      { after: 3000 },
      text('b'),
      { after: 2000 },
      stop(150),
    ])
    await start($)
    written.length = 0

    const { chunks } = await readAll($)

    expect(chunks.map(chunk => chunk.kind)).toEqual(['text', 'stop', 'text', 'stop'])
    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('an answer without a stop says no speed', async ($, on) => {
    const written = scripted(on, [text('a'), { after: 2000 }, text('b')])
    await start($)
    written.length = 0

    const { chunks } = await readAll($)

    expect(chunks.map(chunk => chunk.kind)).toEqual(['text', 'text'])
    expect(written).toEqual([])
  })

  test('a piece after the stop changes nothing', async ($, on) => {
    // The kit lets a test make up no `engine` piece: a thought stands in.
    const written = scripted(on, [text('a'), { after: 2000 }, stop(150), { kind: 'thinking', index: 1, text: 'hm' }])
    await start($)
    written.length = 0

    const { chunks } = await readAll($)

    expect(chunks.map(chunk => chunk.kind)).toEqual(['text', 'stop', 'thinking'])
    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('a turn the user interrupts leaves the speed as it was, and the next answer says its own', async ($, on) => {
    const script: Script = [text('a'), { after: 2000 }, text('b'), { after: 2000 }, stop(300)]
    const written = scripted(on, script)
    await start($)
    written.length = 0

    // The user interrupts after the first piece: the generator is returned
    // into where it stood.
    const stream = $.turn.step({ turnId: 't', index: 0, model: 'opus', messageCount: 1 })
    expect(await stream.next()).toEqual({ done: false, value: text('a') })
    const returned = await stream.return(undefined as never)
    expect(returned.done).toBe(true)
    await idle()
    expect(written).toEqual([])

    // The plugin is not left in a state of its own: the next answer counts.
    script.length = 0
    script.push(text('c'), { after: 2000 }, stop(150))
    await readAll($)
    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('a stream that breaks off says no speed and the plugin goes on', async ($, on) => {
    const script: Script = [text('a'), { after: 2000 }, { breaks: 'the connection broke' }]
    const written = scripted(on, script)
    await start($)
    written.length = 0

    const { chunks, broke } = await readAll($)

    expect(chunks.map(chunk => chunk.kind)).toEqual(['text'])
    // The kit reports the break as its own hook skipped and nothing beneath.
    expect(broke).toBeDefined()
    expect(written).toEqual([])

    script.length = 0
    script.push(text('c'), { after: 2000 }, stop(150))
    await readAll($)
    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('a request sent again after it failed half way counts from the first attempt', async ($, on) => {
    // What the engine says of an answer it begins anew is a piece the
    // plugin cannot read, so it cannot tell the first attempt, the five
    // seconds in which that one died and the engine waited, and the new
    // start from one answer: 19 where the model did 75. The README says
    // so. (A test can make up no piece of the kind `engine`: thoughts
    // stand in for them, which the plugin reads no differently.)
    const written = scripted(on, [
      { kind: 'thinking', index: 0, text: '' },
      text('a'),
      { after: 5000 },
      { kind: 'thinking', index: 1, text: '' },
      { kind: 'thinking', index: 2, text: '' },
      { after: 1000 },
      text('b'),
      { after: 2000 },
      stop(150),
    ])
    await start($)
    written.length = 0

    await readAll($)

    expect(written).toEqual([`${FILE}\n${fast(19)}`])
  })

  test('an answer begins with its first piece, of whatever kind', async ($, on) => {
    // What the model thinks is counted and mostly not streamed: an answer
    // that began with its text would count those tokens over too short a
    // time. (In a session the first piece is of the kind `engine`, which
    // a test cannot make up: a thought stands in.)
    const written = scripted(on, [{ kind: 'thinking', index: 0, text: '' }, { after: 1000 }, text('a'), { after: 1000 }, stop(150)])
    await start($)
    written.length = 0

    await readAll($)

    expect(written).toEqual([`${FILE}\n${fast(75)}`])
  })

  test('the speed of an answer does not come after that of a later one', async ($, on) => {
    // The clock is slow to say when the first answer began, and says so
    // only when the second answer has said its speed.
    let tell: (at: number) => void = () => {}
    const late = new Promise<number>(resolve => {
      tell = resolve
    })
    let asks = 0
    const script: Script = [text('a'), { after: 2000 }, stop(150)]
    const written = scripted(on, script, {}, now => ((asks += 1) === 1 ? late : now))
    await start($)
    written.length = 0

    await readAll($)
    expect(written).toEqual([])
    script.length = 0
    script.push(text('b'), { after: 1000 }, stop(80))
    await readAll($)
    expect(written).toEqual([`${FILE}\n${fast(80)}`])

    tell(0)
    await idle()
    await idle()
    expect(written).toEqual([`${FILE}\n${fast(80)}`])
  })
})
