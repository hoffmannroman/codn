// What codn shows in the bar along its bottom edge, and in the rows of the
// agents below a session what each of them is doing, written to the file
// codn names in CODN_STATUS. The format is codn's, a few keys of TOML; the
// README of codn has them. Outside codn the variable is not set and
// nothing is written.

import type { EngineInterface, Register, SessionContextUsage, SessionRateLimit, TurnStepResult } from 'claude-code'

type Figures = {
  context: SessionContextUsage
  rateLimits: SessionRateLimit[]
}

// The names the limits are shown under. One that is not here keeps its own.
const SHORT = new Map([
  ['five_hour', '5h'],
  ['seven_day', '7d'],
])

// What codn takes at most: for a part in percent, for a count of tokens,
// for tokens a second, of a name and of limits, of how many agents it
// shows what they are doing, and how long that is. codn reads no report
// that could nest deeper than it can follow, and counts for that every
// dot, brace and bracket, also those in a name: a name with more of them
// than this is none, so that what else there is to say is still read.
const MOST_USED = 999
const MOST_TOKENS = 999e6
const MOST_SPEED = 99999
const MOST_NAME = 48
const MOST_OPENERS = 8
const MOST_LIMITS = 4
const MOST_AGENTS = 32
const MOST_DOING = 48

// Text as TOML takes it between quotes: nothing that steers a terminal, and
// no half of a character.
const quoted = (text: string): string => JSON.stringify(text.replace(/[\p{Cc}\p{Cs}]/gu, ''))

// A name as codn shows it: of what is seen, in whole characters, and no
// longer than codn takes. Nothing for what is no name, and for one codn
// would not read.
const named = (text: unknown): string | undefined => {
  if (typeof text !== 'string') return undefined

  const name = [...text.replace(/[\p{Cc}\p{Cs}]/gu, '').trim()].slice(0, MOST_NAME).join('')
  const openers = name.match(/[.{[]/g)?.length ?? 0

  return name !== '' && openers <= MOST_OPENERS ? name : undefined
}

// A name between quotes. Three quotes in a row begin a text of several
// lines, and codn reads no agents behind a report that could hold one: a
// quote of that kind is written as its number.
const quotedName = (name: string): string => quoted(name).replace(/'/g, '\\u0027')

const isShown = (value: number | undefined): value is number =>
  value !== undefined && Number.isFinite(value) && value >= 0

// A whole number of tokens, or of seconds.
const countLine = (key: string, value: number | undefined, most: number): string[] =>
  isShown(value) && value <= most ? [`${key} = ${Math.round(value)}`] : []

// When a limit is whole again, in seconds since 1970 as codn reads it.
const secondsOf = (resetsAt: string | undefined): number | undefined =>
  resetsAt === undefined ? undefined : Date.parse(resetsAt) / 1000

// The limits by the name each is shown under, as many as codn shows. Of
// two by one name the first stands: TOML takes no table twice.
const limitsOf = (limits: SessionRateLimit[]): string[] => {
  const tables = new Map<string, string[]>()
  for (const limit of limits) {
    const name = named(SHORT.get(limit.kind) ?? limit.kind)
    const isTaken = name === undefined || tables.has(name) || tables.size === MOST_LIMITS
    if (isTaken || !isShown(limit.percentUsed)) continue

    tables.set(name, [
      '',
      `[limits.${quotedName(name)}]`,
      `used = ${Math.min(limit.percentUsed, MOST_USED)}`,
      ...countLine('resets', secondsOf(limit.resetsAt), Number.MAX_SAFE_INTEGER),
    ])
  }

  return [...tables.values()].flat()
}

// How hard the model thinks, from the settings: what is set for this model,
// else for every model. A session started with another effort says so when
// its first turn ends.
const effortOf = (settings: Record<string, unknown>, model: string): string | undefined => {
  const perModel = settings.modelSettings
  const forModel =
    typeof perModel === 'object' && perModel !== null ? (perModel as Record<string, unknown>)[model] : undefined
  const own = typeof forModel === 'object' && forModel !== null ? (forModel as Record<string, unknown>).effortLevel : undefined
  const level = own ?? settings.effortLevel

  return typeof level === 'string' && level !== '' ? level : undefined
}

// What the session runs with besides the model: its effort, and that it
// thinks, which it does unless the settings say no.
const optionsOf = (settings: Record<string, unknown>, model: string, effort: string | undefined): string[] => [
  ...[effort ?? effortOf(settings, model)].filter(level => level !== undefined),
  ...(settings.alwaysThinkingEnabled === false ? [] : ['think']),
]

// What a report is made of: what the session said of itself when it was
// asked last.
type Said = {
  model: string
  options: string[]
  figures: Figures
}

// The model and the options under the names codn reads, each left out
// where it has none. The speed stands before the limits: behind them it
// would be a key of the last one.
const reportOf = ({ model, options, figures }: Said, speed: number | undefined): string => {
  const runsWith = options.map(named).filter(option => option !== undefined)

  return [
    ...[named(model)].filter(name => name !== undefined).map(name => `model = ${quotedName(name)}`),
    ...(runsWith.length > 0 ? [`options = [${runsWith.map(quotedName).join(', ')}]`] : []),
    ...countLine('tokens', figures.context.tokens, MOST_TOKENS),
    ...countLine('window', figures.context.window, MOST_TOKENS),
    ...countLine('speed', speed, MOST_SPEED),
    ...limitsOf(figures.rateLimits),
    '',
  ].join('\n')
}

// An answer shorter than this says no speed: over less than a second it is
// that of a few pieces as they happened to arrive.
const LEAST_ANSWER = 1000

// What is known of one answer of the model: which one it is of those the
// session got, when its first piece came and when it was whole, both as
// the clock of the engine has it, and how many tokens it is.
type Answer = {
  order: number
  began: Promise<number | undefined>
  ended: Promise<number | undefined>
  tokens: number | undefined
}

// The file codn named the session, where it named one.
const namedOf = async ($: EngineInterface): Promise<string | undefined> => {
  try {
    const file = await $.env.get('CODN_STATUS')

    return file === '' ? undefined : file
  } catch {
    return undefined
  }
}

// The time now, asked and not waited for: the answer does not wait for
// its clock. Nothing where the clock does not say.
const momentOf = async ($: EngineInterface): Promise<number | undefined> => {
  try {
    return await $.clock.now()
  } catch {
    return undefined
  }
}

// How long, within an answer that came from `from` to `to`, the API ran
// tools of its own, in which the model wrote nothing. Of each run what
// lies in the answer, and a time in which two ran once: a request can
// hold the tools of an answer before this one. A tool whose result did
// not arrive ran to the end of the answer, as in a turn that pauses.
const awayOf = (uses: TurnStepResult['serverToolUses'], from: number, to: number): number => {
  const runs = (uses ?? [])
    .map(use => [Math.max(use.startedAt, from), Math.min(use.endedAt ?? to, to)] as const)
    .filter(([start, end]) => end > start)
    .sort(([a], [b]) => a - b)
  let [away, until] = [0, from]
  for (const [start, end] of runs) {
    if (end <= until) continue

    away += end - Math.max(start, until)
    until = end
  }

  return away
}

// The tokens a second an answer came at, from its first piece to its last,
// as the whole number codn shows. Nothing for an answer that does not say
// how many tokens it is, or when it began and ended, for one too short to
// say, and for a speed codn would not show.
const speedOf = async (
  { began, ended, tokens }: Answer,
  uses: TurnStepResult['serverToolUses'],
): Promise<number | undefined> => {
  const [from, to] = [await began, await ended]
  if (from === undefined || to === undefined || !isShown(tokens)) return undefined

  const took = to - from - awayOf(uses, from, to)
  if (!(took >= LEAST_ANSWER)) return undefined

  const speed = Math.round((tokens * 1000) / took)

  return speed <= MOST_SPEED ? speed : undefined
}

// A text of one line, of the length codn shows at the most.
const oneLine = (text: unknown): string | undefined => {
  if (typeof text !== 'string') return undefined

  const line = text.replace(/[\p{Cc}\p{Cs}\s]+/gu, ' ').trim()
  if (line === '') return undefined

  return line.length > MOST_DOING ? `${line.slice(0, MOST_DOING - 1)}\u2026` : line
}

// A word and what it is said of, where there is something to say it of.
const wordOf = (word: string, of: unknown): string => oneLine(`${word} ${oneLine(of) ?? ''}`) ?? word

// The name of a file, without the folders it lies in.
const fileOf = (path: unknown): string | undefined =>
  typeof path === 'string' ? path.split('/').filter(part => part !== '').pop() : undefined

// One of the names the name of a machine is made of.
const isLabel = (label: string): boolean =>
  /^[a-z0-9-]+$/i.test(label) && !label.startsWith('-') && !label.endsWith('-')

// Whether a text is a machine as an address names it: names joined by
// dots, of which one may stand at the end, or a number in brackets, and
// behind either the port it may have. The name is cut at its dots and
// each part held against a pattern by itself. One pattern for the whole
// name repeats what itself repeats, and on a long text that is no machine
// it tries every way to cut it, twice as many with every letter.
const isMachine = (machine: string): boolean => {
  const name = machine.replace(/:\d{1,5}$/, '')
  if (/^\[[0-9a-f:.]+\]$/i.test(name)) return true

  const labels = name.split('.')
  if (labels.length > 1 && labels.at(-1) === '') labels.pop()

  return labels.every(isLabel)
}

// The machine an address names, without the name and the password an
// address can carry before it, and nothing of what is no address. What
// else can stand in the place of a machine is not written.
const hostOf = (url: unknown): string | undefined => {
  const place = typeof url === 'string' ? /^[a-z][a-z0-9+.-]*:\/\/([^/?#]*)/i.exec(url)?.[1] : undefined
  const machine = place?.slice(place.lastIndexOf('@') + 1)

  return machine !== undefined && isMachine(machine) ? machine : undefined
}

// What the call of a tool says an agent is doing, in a few words made of
// the call: for a command the words the model writes of it, for a file its
// name. Nothing for a tool whose call says nothing a person would read.
// The command itself is never written: it can be made of a secret, and
// what is written here stays in a file.
const doingOf = (tool: string, input: Record<string, unknown>): string | undefined => {
  switch (tool) {
    case 'Bash':
      return oneLine(input.description) ?? tool
    case 'Read':
    case 'Edit':
    case 'Write':
      return wordOf(tool, fileOf(input.file_path))
    case 'NotebookEdit':
      return wordOf('Edit', fileOf(input.notebook_path))
    case 'Grep':
    case 'Glob':
      return wordOf(tool, input.pattern)
    case 'WebFetch':
      return wordOf('Fetch', hostOf(input.url))
    case 'WebSearch':
      return wordOf('Search', input.query)
    case 'Agent':
      return wordOf('Agent', input.description)
    case 'Skill':
      return wordOf('Skill', input.skill)
    default: {
      // A tool of a server: the server and the tool.
      const [, server, name] = /^mcp__(.+?)__(.+)$/.exec(tool) ?? []

      return server === undefined ? undefined : oneLine(`${server}: ${name}`)
    }
  }
}

// The id of an agent as the terminal is told it, which is how codn knows
// the row of that agent: Claude Code makes the id of the record of the id
// of the agent this way, and this follows it, not what codn would take.
const recordOf = (agentId: string): string => agentId.replace(/[^A-Za-z0-9_.+-]/g, '-').slice(0, 32)

// What the agents are doing, as a table by their ids: last in the report
// and a line to each, which is how codn reads it.
const agentsOf = (agents: Map<string, string>): string =>
  agents.size === 0
    ? ''
    : ['', '[agents]', ...[...agents].map(([id, doing]) => `${quoted(id)} = ${quoted(doing)}`), ''].join('\n')

// The figures as they are once the conversation was cleared: the limits
// stay, the tokens go.
const clearedOf = ({ context, rateLimits }: Figures): Figures => ({ context: { window: context.window }, rateLimits })

// What the plugin knows of the session from one event to the next.
type Known = {
  // Whether a person is at the prompt of this session. A `claude -p` that
  // something in a session of codn starts inherits the session's file, and
  // must not put its own figures over those of the program codn started.
  isAttended: boolean
  // Whether codn named the session a file when it started. Outside codn
  // nothing is written, and no answer is timed.
  isNamed: boolean
  // The effort the last turn ran with, once one has ended.
  effort?: string
  // What each agent the session runs is doing, by the id of its record,
  // the one that said so last at the end.
  agents: Map<string, string>
  // What the session said of itself last, of which its report is made,
  // and how many reports were asked for when it said so: one that was
  // asked for earlier and comes later is not put over it.
  said?: Said
  // The tokens a second the last answer of the model came at that had a
  // speed to say. It stays while the session rests. How many answers
  // were timed, and the how manieth of them the speed is of: one that
  // says its speed late is not put over a later one.
  speed?: number
  answers: number
  measured: number
  asked: number
  reported: number
  // A write is out, and what there is to write changed while it was.
  isWriting: boolean
  isDue: boolean
  // The text that was written last, where its write came back whole.
  written?: string
}

// Writes all that is known now: the report of the session with the speed
// of its last answer and, behind it, what its agents are doing. One write
// at a time, and each of what is known when it begins: so an older text
// cannot land on a newer one, and what changes while a write is out is
// written when it is back, also when that write failed. One that cannot
// be written must never get in the way of the session, and is not tried
// again for itself: the next report that is made writes it. A text that
// says what was written last is not written again: the end of a turn
// is three reports, of which mostly none says anything new.
const write = async ($: EngineInterface, known: Known): Promise<void> => {
  if (known.isWriting) {
    known.isDue = true
    return
  }

  known.isWriting = true
  try {
    do {
      known.isDue = false
      try {
        const file = await $.env.get('CODN_STATUS')
        if (file === undefined || file === '' || known.said === undefined) return

        const text = reportOf(known.said, known.speed) + agentsOf(known.agents)
        if (text === known.written) continue

        known.written = undefined
        await $.fs.write(file, text)
        known.written = text
      } catch {
        // Nothing to do: codn goes on showing what it read last.
      }
    } while (known.isDue)
  } finally {
    known.isWriting = false
  }
}

// Makes the report of the session anew, with the figures at hand or else
// those of the session, and writes it. It does not wait for the write:
// a disk that is slow to answer must not hold the start of a session or
// the end of a turn.
const report = async (
  $: EngineInterface,
  known: Known,
  figuresOf: (figures: Figures) => Figures,
  figures?: Figures,
): Promise<void> => {
  if (!known.isAttended) return

  const asked = ++known.asked
  try {
    const file = await $.env.get('CODN_STATUS')
    if (file === undefined || file === '') return

    const model = await $.session.model()
    const options = optionsOf(await $.settings.read(), model, known.effort)
    const said = { model, options, figures: figuresOf(figures ?? (await $.session.usage())) }
    if (asked < known.reported) return

    known.reported = asked
    known.said = said
  } catch {
    // Nothing to do: codn goes on showing what it read last.
    return
  }

  void write($, known)
}

const asTheyAre = (figures: Figures): Figures => figures

// Writes what the agents are doing, or the speed of an answer, with the
// report as it is known, which costs no question to the session: an agent
// calls tools by the dozen, and a turn is many answers. Before the first
// report there is nothing to write it with, and none is made for it: the
// next one of the session has both.
const reportBeside = ($: EngineInterface, known: Known): Promise<void> =>
  known.isAttended && known.said !== undefined ? write($, known) : Promise.resolve()

// Says the speed of an answer once its clock has said its times, where it
// has one to say. One that has none leaves the speed of the answer before,
// and so does one whose clock was slower than a later answer. The same
// speed again is not written again.
const reportSpeed = async (
  $: EngineInterface,
  known: Known,
  answer: Answer,
  uses: TurnStepResult['serverToolUses'],
): Promise<void> => {
  try {
    const speed = await speedOf(answer, uses)
    if (speed === undefined || answer.order < known.measured) return

    known.measured = answer.order
    if (speed === known.speed) return

    known.speed = speed
    await reportBeside($, known)
  } catch {
    // Nothing to do: codn goes on showing the speed it read last.
  }
}

export const register: Register = on => {
  const known: Known = {
    isAttended: false,
    isNamed: false,
    agents: new Map(),
    answers: 0,
    measured: 0,
    asked: 0,
    reported: 0,
    isWriting: false,
    isDue: false,
  }

  on('session.start', async ($, e, next) => {
    const started = await next(e)
    known.isAttended = e.isInteractive
    known.isNamed = (await namedOf($)) !== undefined
    await report($, known, asTheyAre)

    return started
  })

  // After every turn of the main loop, and when a limit moved a point.
  on('session.measure', async ($, e, next) => {
    const measured = await next(e)
    await report($, known, asTheyAre, e)

    return measured
  })

  // The model may have been changed in a turn that moved no figure. The
  // turn of an agent that ends is the end of that agent, which does not
  // wait for the write that takes it out.
  on('turn.complete', async ($, e, next) => {
    const completed = await next(e)
    if (e.agentId === undefined) await report($, known, asTheyAre)
    else if (known.agents.delete(recordOf(e.agentId))) void reportBeside($, known)

    return completed
  })

  // An answer of the model, as it arrives: when its first piece came, when
  // it was whole and how many tokens it is, which is its speed. Every
  // piece is passed on as it came, and none waits for anything here. An
  // answer to an agent is passed on unread: several of them arrive at a
  // time, and the bar speaks of the session. So is every answer where
  // no speed would be written: outside codn, and with nobody at the
  // session.
  on('turn.step', async function* ($, e, next) {
    if (e.agentId !== undefined || !known.isAttended || !known.isNamed) return yield* next(e)

    const stream = next(e)
    let began: Promise<number | undefined> | undefined
    let answer: Answer | undefined
    for await (const chunk of stream) {
      began ??= momentOf($)
      if (chunk.kind === 'stop') {
        // A request can be answered anew: the last answer is the one.
        answer = { order: ++known.answers, began, ended: momentOf($), tokens: chunk.usage?.output_tokens }
        began = undefined
      }
      yield chunk
    }

    const step = await stream.result
    if (answer !== undefined) void reportSpeed($, known, answer, step.serverToolUses)

    return step
  })

  // An agent begins: what one of its id did before, it is not doing.
  on('classic.SubagentStart', async ($, e, next) => {
    try {
      if (known.agents.delete(recordOf(e.agent_id))) void reportBeside($, known)
    } catch {
      // Nothing to do: the first tool it calls says what it is doing.
    }

    return next(e)
  })

  // An agent calls a tool: that is what it is doing now. Said as the tool
  // begins, which can run long, and never in the way of the call: the
  // call does not wait for the write, and it is passed on unread where
  // nothing would be written of it, outside codn and with nobody at the
  // session.
  on('tool.call', async ($, e, next) => {
    if (!known.isAttended || !known.isNamed) return next(e)

    try {
      const id = recordOf(e.agentId ?? '')
      const doing = id === '' ? undefined : doingOf(String(e.tool), e as unknown as Record<string, unknown>)
      if (doing !== undefined && known.agents.get(id) !== doing) {
        // The latest at the end, and where there are too many the one
        // nothing was said of for longest goes.
        known.agents.delete(id)
        known.agents.set(id, doing)
        for (const oldest of [...known.agents.keys()].slice(0, -MOST_AGENTS)) known.agents.delete(oldest)
        void reportBeside($, known)
      }
    } catch {
      // Nothing to do: the row of the agent keeps what it showed.
    }

    return next(e)
  })

  // When the main loop stops, the engine says what effort the turn ran
  // with: the one of the session, whatever set it, and not that of a
  // subagent.
  on('classic.Stop', async ($, e, next) => {
    const stopped = await next(e)
    if (e.agent_id === undefined && e.effort !== undefined) {
      known.effort = e.effort.level
      await report($, known, asTheyAre)
    }

    return stopped
  })

  // After /clear the context is empty, and nothing measures it until the
  // next turn has ended.
  on('session.end', async ($, e, next) => {
    if (e.reason === 'clear') {
      // The agents of the conversation that ends are no agents of the next.
      known.agents.clear()
      await report($, known, clearedOf)
    }

    return next(e)
  })
}
