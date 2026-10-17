// What a session of opencode says to codn, made of the events opencode
// tells its plugins: the report codn shows in the bar along its bottom
// edge and in the rows of the agents below a session, the state of the
// session and of its agents in the reports of OSC 7501, and which
// conversation the session is in. Nothing here knows opencode's plugin
// API or a file: `tui.ts` hands in the events and writes what comes out.
// The format of the report is codn's, a few keys of TOML; the README of
// codn has them.

// What codn takes at most: for a count of tokens, for tokens a second,
// of a name, of how many agents it shows what they are doing, and how
// long that is. codn reads no report that could nest deeper than it can
// follow, and counts for that every dot, brace and bracket, also those in
// a name: a name with more of them than this is none, so that what else
// there is to say is still read.
export const MOST_TOKENS = 999e6
export const MOST_SPEED = 99999
export const MOST_NAME = 48
export const MOST_OPENERS = 8
export const MOST_AGENTS = 32
export const MOST_DOING = 48

// What codn takes of a record: the length of a part of its id, and of the
// text of its title before that is put in base64.
export const MOST_ID = 32
export const MOST_TITLE = 192

// How many sessions above an agent are looked through for the session
// shown: an agent opencode sends off can send off another.
const MOST_DEPTH = 8

// An answer shorter than this says no speed: over less than a second it is
// that of a few pieces as they happened to arrive.
const LEAST_ANSWER = 1000

// The name codn's reports carry for the program that sent them.
const APP = 'opencode'

// Text as TOML takes it between quotes: nothing that steers a terminal, and
// no half of a character.
const quoted = (text: string): string => JSON.stringify(text.replace(/[\p{Cc}\p{Cs}]/gu, ''))

// A name as codn shows it: of what is seen, in whole characters, and no
// longer than codn takes. Nothing for what is no name, and for one codn
// would not read.
export const named = (text: unknown): string | undefined => {
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

// A whole number of tokens, or of tokens a second.
const countLine = (key: string, value: number | undefined, most: number): string[] =>
  isShown(value) && value <= most ? [`${key} = ${Math.round(value)}`] : []

// A number an event says, or nothing where it says none.
const numberOf = (value: unknown): number | undefined =>
  typeof value === 'number' && Number.isFinite(value) ? value : undefined

const textOf = (value: unknown): string | undefined => (typeof value === 'string' && value !== '' ? value : undefined)

const recordOf = (value: unknown): Record<string, unknown> =>
  typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {}

// A text on one line, with nothing that steers a terminal.
const flat = (text: unknown): string | undefined => {
  if (typeof text !== 'string') return undefined

  const line = text.replace(/[\p{Cc}\p{Cs}\s]+/gu, ' ').trim()
  return line === '' ? undefined : line
}

// A text of one line, of the length codn shows at the most.
const oneLine = (text: unknown): string | undefined => {
  const line = flat(text)
  return line !== undefined && line.length > MOST_DOING ? `${line.slice(0, MOST_DOING - 1)}…` : line
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
// each part held against a pattern by itself: one pattern for the whole
// name would try every way to cut a long text that is no machine.
const isMachine = (machine: string): boolean => {
  const name = machine.replace(/:\d{1,5}$/, '')
  if (/^\[[0-9a-f:.]+\]$/i.test(name)) return true

  const labels = name.split('.')
  if (labels.length > 1 && labels.at(-1) === '') labels.pop()

  return labels.every(isLabel)
}

// The machine an address names, without the name and the password an
// address can carry before it, and nothing of what is no address.
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
export const doingOf = (tool: string, input: Record<string, unknown>): string | undefined => {
  const file = input.filePath ?? input.path ?? input.file
  switch (tool) {
    case 'shell':
      return oneLine(input.description) ?? 'Shell'
    case 'read':
      return wordOf('Read', fileOf(file))
    case 'edit':
    case 'write':
    case 'patch':
      return wordOf('Edit', fileOf(file))
    case 'grep':
      return wordOf('Grep', input.pattern)
    case 'glob':
      return wordOf('Glob', input.pattern)
    case 'webfetch':
      return wordOf('Fetch', hostOf(input.url))
    case 'websearch':
      return wordOf('Search', input.query)
    case 'subagent':
      return wordOf('Agent', input.description)
    case 'skill':
      return wordOf('Skill', input.name)
    default:
      return undefined
  }
}

// The id of a record as codn takes it: of the characters it allows, and
// no longer. opencode's ids of sessions are made of them already.
export const recordIdOf = (id: string): string => id.replace(/[^A-Za-z0-9_.+-]/g, '-').slice(0, MOST_ID)

// A text in base64 as a report carries it, cut to what codn takes, in
// whole characters.
const base64Of = (text: string): string => {
  const bytes = new TextEncoder().encode(text)
  let end = Math.min(bytes.length, MOST_TITLE)
  // A byte that continues a character is not where one may be cut.
  while (end > 0 && end < bytes.length && (bytes[end] & 0xc0) === 0x80) end--

  return Buffer.from(bytes.subarray(0, end)).toString('base64')
}

export type State = 'idle' | 'working' | 'blocked' | 'done' | 'error'
export type Kind = 'permission' | 'question'

// A report of OSC 7501: of the session where it has no id, of one of its
// agents where it has. Nothing in it steers a terminal but its frame.
export const oscOf = (fields: { state: State | 'clear'; id?: string; kind?: Kind; title?: string }): string => {
  const parts = [`state=${fields.state}`, `app=${APP}`]
  if (fields.id !== undefined) parts.push(`id=${recordIdOf(fields.id)}`)
  if (fields.kind !== undefined) parts.push(`kind=${fields.kind}`)
  const title = flat(fields.title)
  if (title !== undefined) parts.push(`title=${base64Of(title)}`)

  return `\x1b]7501;${parts.join(':')}\x1b\\`
}

// The question whether a terminal takes these reports, and its answer.
export const QUESTION = '\x1b]7501;?\x1b\\'
export const isAnswer = (sequence: string): boolean => /^\x1b\]7501;\?(\x1b\\|\x07)$/.test(sequence)

export type Model = { id: string; provider?: string; variant?: string }

// An agent the session sent off: the session above it, and the few words
// it was sent off with.
type Agent = { parent: string; title?: string }

// A question to the user that stands: of which session, and of what kind.
type Waiting = { session: string; kind: Kind }

// How many calls, questions and conversations are kept at the most. The
// oldest goes first: what a session does is rarely so much at once.
const MOST_KEPT = 256

// What the plugin knows of the session from one event to the next.
export type Known = {
  // The conversation the window of opencode shows.
  shown?: string
  // The conversations it showed: what they and the agents they sent off
  // do is this session's. Another program can share opencode's server.
  seen: Set<string>
  model?: Model
  agent?: string
  tokens?: number
  window?: number
  // The tokens a second the last answer of the model came at that had a
  // speed to say. It stays while the session rests.
  speed?: number
  // When the first and the last piece of each answer that is under way
  // came: the model writes from the one to the other, and the answer ends
  // later where it called a tool, which then runs.
  pieces: Map<string, { first: number; last: number }>
  // The tools that are called, by the id of their call: the name each is
  // called by and of which session, and for an agent that is sent off the
  // words it is sent off with.
  tools: Map<string, { name: string; session?: string }>
  sending: Map<string, string | undefined>
  // The agents sent off that have not ended, by their sessions.
  agents: Map<string, Agent>
  // What each agent is doing, by its session, the one that said so last
  // at the end.
  doing: Map<string, string>
  // The questions to the user that stand, by their ids: a permission or a
  // question of opencode's. The session waits while one of its own does.
  waiting: Map<string, Waiting>
  // The state the record of the session was given last.
  state?: State
  kind?: Kind
}

export const knownOf = (): Known => ({
  seen: new Set(),
  pieces: new Map(),
  tools: new Map(),
  sending: new Map(),
  agents: new Map(),
  doing: new Map(),
  waiting: new Map(),
})

// What an event leads to: the report is to be written anew, and these
// reports of OSC 7501 are to be sent.
export type Outcome = { report: boolean; records: string[] }

const nothing = (): Outcome => ({ report: false, records: [] })

// Puts a value last, where the latest stand, and lets the oldest go where
// there are more than `most`. Says what went.
const keep = <K, V>(map: Map<K, V>, key: K, value: V, most: number): K[] => {
  map.delete(key)
  map.set(key, value)
  const gone = [...map.keys()].slice(0, Math.max(0, map.size - most))
  for (const key of gone) map.delete(key)
  return gone
}

// Whether a conversation is one the window showed or an agent one of them
// sent off, at whatever depth below it, as far as that is looked.
const isOurs = (known: Known, session: string | undefined): boolean => {
  for (let at = session, depth = 0; at !== undefined && depth <= MOST_DEPTH; depth++) {
    if (known.seen.has(at)) return true
    at = known.agents.get(at)?.parent
  }
  return false
}

// The record of the session in another state, where it is another.
const said = (known: Known, state: State, kind?: Kind): string[] => {
  if (known.state === state && known.kind === kind) return []

  known.state = state
  known.kind = kind
  return [oscOf({ state, kind })]
}

// The state of a session at work: it waits while a question of its own
// stands, the one asked last, and works otherwise.
const atWork = (known: Known): string[] => {
  const asked = [...known.waiting.values()].findLast(waiting => isOurs(known, waiting.session))
  return asked === undefined ? said(known, 'working') : said(known, 'blocked', asked.kind)
}

// An agent has ended, or gone for a later one: its record goes, what it
// was doing, and the questions it asked.
const ended = (known: Known, session: string): string[] => {
  known.agents.delete(session)
  known.doing.delete(session)
  for (const [id, waiting] of known.waiting) if (waiting.session === session) known.waiting.delete(id)
  return [oscOf({ state: 'clear', id: session })]
}

// What the window shows now: another conversation, or its start page. The
// figures of the one before are not those of the next; the agents it sent
// off still run in this session, and keep their records. `running` says
// whether the conversation is at work.
export const show = (known: Known, session: string | undefined, running: boolean): Outcome => {
  if (session === known.shown) return nothing()

  Object.assign(known, { shown: session, model: undefined, agent: undefined, tokens: undefined })
  Object.assign(known, { window: undefined, speed: undefined, state: undefined, kind: undefined })
  known.pieces.clear()
  if (session === undefined) return { report: true, records: said(known, 'idle') }

  known.seen.delete(session)
  known.seen.add(session)
  for (const oldest of [...known.seen].slice(0, Math.max(0, known.seen.size - MOST_KEPT))) known.seen.delete(oldest)
  return { report: true, records: running ? atWork(known) : said(known, 'idle') }
}

// The figures of the context, from what the model was asked with last.
export const tokensOf = (tokens: unknown): number | undefined => {
  const counts = recordOf(tokens)
  const cache = recordOf(counts.cache)
  const input = numberOf(counts.input)
  if (input === undefined) return undefined

  return input + (numberOf(cache.read) ?? 0) + (numberOf(cache.write) ?? 0)
}

// What is known of a conversation the session shows that it did not see
// begin, a resumed one for one: the model and the agent it last ran with,
// and the context of its last answer. `info` and `messages` are those
// opencode keeps of it, as its plugin is given them.
export const resumed = (
  known: Known,
  session: string,
  info: unknown,
  messages: unknown,
  windowOf: (model: Model) => number | undefined,
): Outcome => {
  if (session !== known.shown) return nothing()

  const said = recordOf(info)
  const model = recordOf(said.model)
  const id = textOf(model.id)
  if (known.model === undefined && id !== undefined) {
    known.model = { id, provider: textOf(model.providerID), variant: textOf(model.variant) }
  }
  known.agent ??= textOf(said.agent)
  if (known.model !== undefined) known.window ??= windowOf(known.model)
  const answers = Array.isArray(messages) ? messages.map(recordOf).filter(m => m.type === 'assistant') : []
  known.tokens ??= answers.map(answer => tokensOf(answer.tokens)).findLast(tokens => tokens !== undefined)

  return { report: true, records: [] }
}

// The tokens a second of an answer, from its first piece to its last, as
// the whole number codn shows. Nothing for one too short to say.
const speedOf = (from: number | undefined, to: number | undefined, tokens: number | undefined): number | undefined => {
  if (from === undefined || to === undefined || !isShown(tokens)) return undefined

  const took = to - from
  if (!(took >= LEAST_ANSWER)) return undefined

  const speed = Math.round((tokens * 1000) / took)
  return speed <= MOST_SPEED ? speed : undefined
}

// Takes in one event of opencode: its type, when it happened by opencode's
// clock, and what it says. `windowOf` tells how many tokens a model takes.
export const take = (
  known: Known,
  type: string,
  created: number | undefined,
  data: Record<string, unknown>,
  windowOf: (model: Model) => number | undefined,
): Outcome => {
  const session = textOf(data.sessionID)
  const isShownOne = session !== undefined && session === known.shown

  switch (type) {
    case 'session.execution.started':
      return isShownOne ? { report: false, records: atWork(known) } : nothing()

    case 'session.execution.succeeded':
    case 'session.execution.failed':
    case 'session.execution.interrupted': {
      // What was under way for it will not end on its own.
      for (const [call, tool] of known.tools) if (tool.session === session) known.tools.delete(call)
      if (session !== undefined && known.agents.has(session)) {
        const report = known.doing.has(session)
        const records = ended(known, session)
        return { report, records: known.state === 'blocked' ? [...records, ...atWork(known)] : records }
      }
      if (!isShownOne) return nothing()

      known.pieces.clear()
      const state = type.endsWith('succeeded') ? 'done' : type.endsWith('failed') ? 'error' : 'idle'
      return { report: false, records: said(known, state) }
    }

    case 'permission.asked':
    case 'form.created': {
      // A question names its session inside the form.
      const form = recordOf(data.form)
      const asking = type === 'form.created' ? (textOf(form.sessionID) ?? session) : session
      const id = textOf(type === 'form.created' ? form.id : data.id)
      if (id === undefined || asking === undefined || !isOurs(known, asking)) return nothing()

      const kind = type === 'form.created' ? 'question' : 'permission'
      keep(known.waiting, id, { session: asking, kind }, MOST_KEPT)
      return { report: false, records: said(known, 'blocked', kind) }
    }

    case 'permission.replied':
    case 'form.replied':
    case 'form.cancelled': {
      const id = textOf(type === 'permission.replied' ? data.requestID : data.id)
      if (id === undefined || !known.waiting.delete(id) || known.state !== 'blocked') return nothing()

      return { report: false, records: atWork(known) }
    }

    case 'session.step.started': {
      if (!isShownOne) return nothing()

      const model = recordOf(data.model)
      const id = textOf(model.id)
      if (id !== undefined) {
        known.model = { id, provider: textOf(model.providerID), variant: textOf(model.variant) }
        known.window = windowOf(known.model)
      }
      known.agent = textOf(data.agent) ?? known.agent
      return { report: true, records: [] }
    }

    case 'session.reasoning.delta':
    case 'session.text.delta':
    case 'session.tool.input.started':
    case 'session.tool.input.ended': {
      const answer = textOf(data.assistantMessageID)
      if (isShownOne && answer !== undefined && created !== undefined) {
        const piece = known.pieces.get(answer)
        if (piece === undefined) keep(known.pieces, answer, { first: created, last: created }, MOST_KEPT)
        else piece.last = Math.max(piece.last, created)
      }
      const call = textOf(data.id)
      const name = textOf(data.name)
      if (type === 'session.tool.input.started' && call !== undefined && name !== undefined) {
        keep(known.tools, call, { name, session }, MOST_KEPT)
      }
      return nothing()
    }

    case 'session.step.ended': {
      if (!isShownOne) return nothing()

      const answer = textOf(data.assistantMessageID)
      const counts = recordOf(data.tokens)
      const written = (numberOf(counts.output) ?? 0) + (numberOf(counts.reasoning) ?? 0)
      const piece = answer === undefined ? undefined : known.pieces.get(answer)
      if (answer !== undefined) known.pieces.delete(answer)

      known.tokens = tokensOf(data.tokens) ?? known.tokens
      known.speed = speedOf(piece?.first, piece?.last, written) ?? known.speed
      return { report: true, records: [] }
    }

    case 'session.tool.called': {
      const call = textOf(data.id)
      const input = recordOf(data.input)
      const tool = (call === undefined ? undefined : known.tools.get(call)?.name) ?? textOf(data.name)
      if (call !== undefined) known.tools.delete(call)
      if (call !== undefined && tool === 'subagent' && isOurs(known, session)) {
        keep(known.sending, call, flat(input.description), MOST_KEPT)
      }
      if (session === undefined || !known.agents.has(session) || tool === undefined) return nothing()

      const doing = doingOf(tool, input)
      if (doing === undefined || known.doing.get(session) === doing) return nothing()

      // The latest at the end, and where there are too many the one
      // nothing was said of for longest goes.
      keep(known.doing, session, doing, MOST_AGENTS)
      return { report: true, records: [] }
    }

    case 'session.tool.progress': {
      // The call that sends an agent off says, as it runs, which session
      // the agent is: from then on it has a record. Where there are more
      // than codn shows, the one sent off first goes.
      const call = textOf(data.id)
      const agent = textOf(recordOf(data.metadata).sessionID)
      if (call === undefined || agent === undefined || session === undefined) return nothing()
      if (!known.sending.has(call) || known.agents.has(agent)) return nothing()

      const title = known.sending.get(call)
      known.sending.delete(call)
      const gone = keep(known.agents, agent, { parent: session, title }, MOST_AGENTS)
      const records = gone.flatMap(oldest => {
        known.agents.set(oldest, { parent: session })
        return ended(known, oldest)
      })
      return { report: gone.length > 0, records: [...records, oscOf({ state: 'working', id: agent, title })] }
    }

    case 'session.tool.success':
    case 'session.tool.failed': {
      const call = textOf(data.id)
      if (call !== undefined) {
        known.sending.delete(call)
        known.tools.delete(call)
      }
      return nothing()
    }

    default:
      return nothing()
  }
}

// What a report says that has nothing to say. An empty file is one its
// program is writing, and codn goes on showing what it read before.
export const NOTHING = '# nothing to report\n'

// The model and the options under the names codn reads, each left out
// where it has none: the agent of opencode it runs as, and the variant of
// the model where one was chosen. The speed stands last: behind a table
// it would be a key of that table.
export const reportOf = (known: Known): string => {
  const variant = known.model?.variant
  const options = [known.agent, variant === 'default' ? undefined : variant].map(named).filter(o => o !== undefined)
  const model = named(known.model?.id)
  const agents =
    known.doing.size === 0
      ? []
      : ['', '[agents]', ...[...known.doing].map(([id, doing]) => `${quoted(recordIdOf(id))} = ${quoted(doing)}`)]

  const lines = [
    ...(model === undefined ? [] : [`model = ${quotedName(model)}`]),
    ...(options.length > 0 ? [`options = [${options.map(quotedName).join(', ')}]`] : []),
    ...countLine('tokens', known.tokens, MOST_TOKENS),
    ...countLine('window', known.window, MOST_TOKENS),
    ...countLine('speed', known.speed, MOST_SPEED),
    ...agents,
  ]
  return lines.length === 0 ? NOTHING : [...lines, ''].join('\n')
}

// What codn reads to know which conversation a session is in: the id
// under the key `id`, as the history of the agent says.
export const runningOf = (known: Known): string | undefined =>
  known.shown === undefined ? undefined : `${JSON.stringify({ id: known.shown })}\n`
