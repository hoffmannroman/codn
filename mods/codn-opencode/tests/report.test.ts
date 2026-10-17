// The tests of what the plugin makes of opencode's events. They run with
// `node --test mods/codn-opencode/tests/`, without opencode: the events are
// made up after those opencode 2.0.31 sent in a session.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import {
  doingOf,
  isAnswer,
  knownOf,
  MOST_AGENTS,
  named,
  NOTHING,
  oscOf,
  QUESTION,
  recordIdOf,
  reportOf,
  resumed,
  runningOf,
  show,
  take,
} from '../report.ts'
import type { Known, Model, Outcome } from '../report.ts'

const SESSION = 'ses_0a1b2c3d4e5fAbCdEfGhIjKlMn'
const AGENT = 'ses_0a1b2c3d4e60NoPqRsTuVwXyZa'
const MODEL = { id: 'big-pickle', providerID: 'opencode', variant: 'default' }
const windowOf = (model: Model): number | undefined => (model.id === 'big-pickle' ? 200000 : undefined)

// A session that shows SESSION, and a way to hand it events.
const shown = (): { known: Known; event: (type: string, data: Record<string, unknown>, created?: number) => Outcome } => {
  const known = knownOf()
  show(known, SESSION, false)
  return { known, event: (type, data, created = 0) => take(known, type, created, data, windowOf) }
}

// The text of a report of OSC 7501 between its frame, and its title read.
const fieldsOf = (record: string): Record<string, string> => {
  const body = /^\x1b\]7501;(.*)\x1b\\$/.exec(record)?.[1]
  assert.ok(body !== undefined, JSON.stringify(record))
  const fields = Object.fromEntries(body.split(':').map(part => [part.slice(0, part.indexOf('=')), part.slice(part.indexOf('=') + 1)]))
  if (fields.title !== undefined) fields.title = Buffer.from(fields.title, 'base64').toString()
  return fields
}

// A turn of the session as opencode tells it: a step of 50 tokens written
// from 1000 to 2000, which ends at 6000, after the tool it called ran.
const turn = (event: ReturnType<typeof shown>['event']) => {
  event('session.execution.started', { sessionID: SESSION })
  event('session.step.started', { sessionID: SESSION, agent: 'build', model: MODEL, assistantMessageID: 'msg_1' })
  event('session.reasoning.delta', { sessionID: SESSION, assistantMessageID: 'msg_1', delta: 'Hm' }, 1000)
  event('session.text.delta', { sessionID: SESSION, assistantMessageID: 'msg_1', delta: 'ok' }, 2000)
  event(
    'session.step.ended',
    {
      sessionID: SESSION,
      assistantMessageID: 'msg_1',
      tokens: { input: 352, output: 40, reasoning: 10, cache: { read: 1536, write: 12 } },
    },
    6000,
  )
}

test('a turn is reported with the model, the agent, the context and the speed', () => {
  const { known, event } = shown()
  turn(event)
  // The text src/core/reported.rs of codn reads in a test of its own, from
  // the same file: change both together.
  const expected = readFileSync(new URL('./report.toml', import.meta.url), 'utf8')
  known.doing.set(AGENT, 'Read HEAD')
  assert.equal(reportOf(known), expected)
})

test('the variant of a model is an option, the default variant not', () => {
  const { known, event } = shown()
  event('session.step.started', { sessionID: SESSION, agent: 'plan', model: { ...MODEL, variant: 'high' } })
  assert.match(reportOf(known), /^options = \["plan", "high"\]$/m)
  event('session.step.started', { sessionID: SESSION, agent: 'plan', model: MODEL })
  assert.match(reportOf(known), /^options = \["plan"\]$/m)
})

test('what another conversation does is not reported', () => {
  const { known, event } = shown()
  event('session.step.started', { sessionID: 'ses_other', agent: 'build', model: MODEL })
  event('session.step.ended', { sessionID: 'ses_other', assistantMessageID: 'm', tokens: { input: 9 } }, 9000)
  assert.equal(reportOf(known), NOTHING)
  assert.deepEqual(event('session.execution.started', { sessionID: 'ses_other' }).records, [])
})

test('a report with nothing to say is no empty file, which codn takes for one being written', () => {
  assert.equal(reportOf(knownOf()), NOTHING)
  assert.match(NOTHING, /^#[^\n]*\n$/)
})

test('an answer too short to say a speed leaves the one before', () => {
  const { known, event } = shown()
  turn(event)
  event('session.text.delta', { sessionID: SESSION, assistantMessageID: 'msg_2', delta: 'x' }, 5000)
  event('session.step.ended', { sessionID: SESSION, assistantMessageID: 'msg_2', tokens: { input: 2000, output: 900 } }, 5400)
  assert.equal(known.speed, 50)
  assert.equal(known.tokens, 2000)
})

test('a conversation that is resumed is reported with what opencode kept of it', () => {
  const known = knownOf()
  show(known, SESSION, false)
  const info = { id: SESSION, agent: 'plan', model: MODEL, tokens: { input: 1, cache: { read: 99999 } } }
  const messages = [
    { type: 'user' },
    { type: 'assistant', tokens: { input: 477, output: 5, cache: { read: 8512, write: 0 } } },
    { type: 'assistant', tokens: { input: 300, cache: { read: 9000 } } },
    { type: 'idle', outcome: 'succeeded' },
  ]
  assert.equal(resumed(known, 'ses_other', info, messages, windowOf).report, false, 'not the one shown')
  assert.equal(resumed(known, SESSION, info, messages, windowOf).report, true)
  assert.equal(reportOf(known), 'model = "big-pickle"\noptions = ["plan"]\ntokens = 9300\nwindow = 200000\n')
  // What the session said since stands.
  known.tokens = 5
  resumed(known, SESSION, { model: { id: 'other' } }, [], windowOf)
  assert.equal(known.tokens, 5)
  assert.equal(known.model?.id, 'big-pickle')
})

test('the session says what it does, and each state once', () => {
  const { event } = shown()
  const states = (outcome: Outcome) => outcome.records.map(record => fieldsOf(record))
  assert.deepEqual(states(event('session.execution.started', { sessionID: SESSION })), [{ state: 'working', app: 'opencode' }])
  assert.deepEqual(states(event('session.execution.started', { sessionID: SESSION })), [], 'said already')
  assert.deepEqual(states(event('permission.asked', { sessionID: SESSION, id: 'per_1' })), [
    { state: 'blocked', app: 'opencode', kind: 'permission' },
  ])
  assert.deepEqual(states(event('permission.replied', { sessionID: SESSION, requestID: 'per_1' })), [{ state: 'working', app: 'opencode' }])
  // A question names its session inside the form, as opencode 2.0.31 sends it.
  const form = { form: { id: 'form_1', sessionID: SESSION, title: 'Question' } }
  assert.deepEqual(states(event('form.created', form)), [{ state: 'blocked', app: 'opencode', kind: 'question' }])
  assert.deepEqual(states(event('form.cancelled', { sessionID: SESSION, id: 'form_1' })), [{ state: 'working', app: 'opencode' }])
  assert.deepEqual(states(event('session.execution.succeeded', { sessionID: SESSION })), [{ state: 'done', app: 'opencode' }])
  event('session.execution.started', { sessionID: SESSION })
  assert.deepEqual(states(event('session.execution.failed', { sessionID: SESSION })), [{ state: 'error', app: 'opencode' }])
  event('session.execution.started', { sessionID: SESSION })
  assert.deepEqual(states(event('session.execution.interrupted', { sessionID: SESSION })), [{ state: 'idle', app: 'opencode' }])
})

test('an agent the session sends off has a record from its start to its end, and says what it does', () => {
  const { known, event } = shown()
  event('session.tool.input.started', { sessionID: SESSION, id: 'call_1', name: 'subagent' })
  event('session.tool.called', { sessionID: SESSION, id: 'call_1', input: { agent: 'explore', description: 'Read the HEAD' } })
  const started = event('session.tool.progress', { sessionID: SESSION, id: 'call_1', metadata: { sessionID: AGENT, status: 'running' } })
  assert.deepEqual(started.records.map(fieldsOf), [{ state: 'working', app: 'opencode', id: AGENT, title: 'Read the HEAD' }])
  assert.deepEqual(event('session.tool.progress', { sessionID: SESSION, id: 'call_1', metadata: { sessionID: AGENT } }).records, [])

  event('session.tool.input.started', { sessionID: AGENT, id: 'call_2', name: 'read' })
  assert.equal(event('session.tool.called', { sessionID: AGENT, id: 'call_2', input: { filePath: '/a/.git/HEAD' } }).report, true)
  assert.match(reportOf(known), /\[agents\]\n"ses_0a1b2c3d4e60NoPqRsTuVwXyZa" = "Read HEAD"\n/)

  // A permission an agent waits for stops the session.
  assert.deepEqual(event('permission.asked', { sessionID: AGENT, id: 'per_1' }).records.map(fieldsOf), [
    { state: 'blocked', app: 'opencode', kind: 'permission' },
  ])

  // Its permission goes with it, and the session works on.
  const ended = event('session.execution.succeeded', { sessionID: AGENT })
  assert.deepEqual(ended.records.map(fieldsOf), [
    { state: 'clear', app: 'opencode', id: AGENT },
    { state: 'working', app: 'opencode' },
  ])
  assert.equal(ended.report, true)
  assert.doesNotMatch(reportOf(known), /agents/)
})

test('the session waits as long as one of the questions to the user stands', () => {
  const { known, event } = shown()
  event('session.execution.started', { sessionID: SESSION })
  event('session.tool.called', { sessionID: SESSION, id: 'call_1', name: 'subagent', input: { description: 'Go' } })
  event('session.tool.progress', { sessionID: SESSION, id: 'call_1', metadata: { sessionID: AGENT } })
  const states = (outcome: Outcome) => outcome.records.map(record => fieldsOf(record))
  assert.deepEqual(states(event('permission.asked', { sessionID: AGENT, id: 'per_1' })), [
    { state: 'blocked', app: 'opencode', kind: 'permission' },
  ])
  assert.deepEqual(states(event('form.created', { form: { id: 'form_1', sessionID: SESSION } })), [
    { state: 'blocked', app: 'opencode', kind: 'question' },
  ])
  assert.deepEqual(states(event('permission.replied', { sessionID: AGENT, requestID: 'per_1' })), [], 'the question stands')
  assert.equal(known.state, 'blocked')
  assert.deepEqual(states(event('permission.replied', { sessionID: AGENT, requestID: 'per_unknown' })), [])
  assert.deepEqual(states(event('form.replied', { sessionID: SESSION, id: 'form_1' })), [{ state: 'working', app: 'opencode' }])
  // A permission that ended with its agent waits no more.
  event('permission.asked', { sessionID: AGENT, id: 'per_2' })
  assert.deepEqual(states(event('session.execution.failed', { sessionID: AGENT })), [
    { state: 'clear', app: 'opencode', id: AGENT },
    { state: 'working', app: 'opencode' },
  ])
})

test('the agents of a conversation stay its own while another is shown, and after', () => {
  const { known, event } = shown()
  event('session.tool.called', { sessionID: SESSION, id: 'call_1', name: 'subagent', input: { description: 'Go' } })
  event('session.tool.progress', { sessionID: SESSION, id: 'call_1', metadata: { sessionID: AGENT } })
  // The agent runs in this terminal whatever its window shows: its row stays.
  assert.deepEqual(show(known, 'ses_next', false).records.map(fieldsOf), [{ state: 'idle', app: 'opencode' }])
  assert.deepEqual(show(known, SESSION, true).records.map(fieldsOf), [{ state: 'working', app: 'opencode' }])
  assert.equal(event('session.tool.called', { sessionID: AGENT, id: 'r', name: 'read', input: { filePath: '/a/b.rs' } }).report, true)
  assert.equal(known.doing.get(AGENT), 'Read b.rs')
  assert.deepEqual(event('permission.asked', { sessionID: AGENT, id: 'per_1' }).records.map(fieldsOf), [
    { state: 'blocked', app: 'opencode', kind: 'permission' },
  ])
  assert.deepEqual(event('session.execution.succeeded', { sessionID: AGENT }).records.map(fieldsOf)[0], {
    state: 'clear',
    app: 'opencode',
    id: AGENT,
  })
})

test('what is kept of calls and answers that went wrong goes with them', () => {
  const { known, event } = shown()
  event('session.tool.input.started', { sessionID: SESSION, id: 'call_1', name: 'subagent' })
  event('session.tool.called', { sessionID: SESSION, id: 'call_1', input: { description: 'Go' } })
  event('session.tool.failed', { sessionID: SESSION, id: 'call_1' })
  assert.equal(known.sending.size, 0)
  event('session.tool.input.started', { sessionID: SESSION, id: 'call_2', name: 'read', assistantMessageID: 'msg_1' }, 10)
  event('session.text.delta', { sessionID: SESSION, assistantMessageID: 'msg_1', delta: 'x' }, 20)
  event('session.execution.interrupted', { sessionID: SESSION })
  assert.equal(known.pieces.size, 0)
  assert.equal(known.tools.size, 0)
  for (let n = 0; n < 1000; n++) event('session.tool.input.started', { sessionID: 'ses_other', id: `c${n}`, name: 'read' })
  assert.ok(known.tools.size <= 256, `${known.tools.size}`)
})

test('an agent another conversation sends off is none of this session', () => {
  const { known, event } = shown()
  event('session.tool.input.started', { sessionID: 'ses_other', id: 'call_1', name: 'subagent' })
  event('session.tool.called', { sessionID: 'ses_other', id: 'call_1', input: { description: 'Elsewhere' } })
  assert.deepEqual(event('session.tool.progress', { sessionID: 'ses_other', id: 'call_1', metadata: { sessionID: AGENT } }).records, [])
  assert.equal(known.agents.size, 0)
})

test('no more agents are said than codn shows, the one said of longest ago goes', () => {
  const { known, event } = shown()
  for (let n = 0; n <= MOST_AGENTS; n++) {
    const agent = `ses_agent${n}`
    event('session.tool.called', { sessionID: SESSION, id: `call_${n}`, name: 'subagent', input: { description: `Agent ${n}` } })
    event('session.tool.progress', { sessionID: SESSION, id: `call_${n}`, metadata: { sessionID: agent } })
    event('session.tool.called', { sessionID: agent, id: `read_${n}`, name: 'grep', input: { pattern: `p${n}` } })
  }
  assert.equal(known.doing.size, MOST_AGENTS)
  assert.ok(!known.doing.has('ses_agent0'))
})

test('no more agents have a record than codn shows, and one that goes is taken back', () => {
  const { known, event } = shown()
  const records: string[] = []
  for (let n = 0; n <= MOST_AGENTS; n++) {
    event('session.tool.called', { sessionID: SESSION, id: `call_${n}`, name: 'subagent', input: { description: `Agent ${n}` } })
    records.push(...event('session.tool.progress', { sessionID: SESSION, id: `call_${n}`, metadata: { sessionID: `ses_agent${n}` } }).records)
  }
  assert.equal(known.agents.size, MOST_AGENTS)
  assert.ok(!known.agents.has('ses_agent0'))
  assert.deepEqual(fieldsOf(records.at(-2) ?? ''), { state: 'clear', app: 'opencode', id: 'ses_agent0' })
})

test('another conversation shown takes back what was said of the one before', () => {
  const { known, event } = shown()
  turn(event)
  const next = show(known, 'ses_next', true)
  assert.deepEqual(next.records.map(fieldsOf), [{ state: 'working', app: 'opencode' }])
  assert.equal(reportOf(known), NOTHING)
  assert.equal(runningOf(known), '{"id":"ses_next"}\n')
  const home = show(known, undefined, false)
  // The start page is a session at rest. A clear of the session could take
  // the records of agents that still run with it.
  assert.deepEqual(home.records.map(fieldsOf), [{ state: 'idle', app: 'opencode' }])
  assert.equal(runningOf(known), undefined)
  assert.deepEqual(show(known, undefined, false), { report: false, records: [] }, 'nothing changed')
})

test('the calls of tools are said in a few words, and never the command', () => {
  assert.equal(doingOf('shell', { command: 'curl -u me:secret x', description: 'Fetch the page' }), 'Fetch the page')
  assert.equal(doingOf('shell', { command: 'curl -u me:secret x' }), 'Shell')
  assert.equal(doingOf('read', { filePath: '/a/b/main.rs' }), 'Read main.rs')
  assert.equal(doingOf('patch', { path: 'src/x.ts' }), 'Edit x.ts')
  assert.equal(doingOf('grep', { pattern: 'fn main' }), 'Grep fn main')
  assert.equal(doingOf('webfetch', { url: 'https://me:pw@example.org/a?b' }), 'Fetch example.org')
  assert.equal(doingOf('webfetch', { url: 'not an address' }), 'Fetch')
  assert.equal(doingOf('subagent', { description: 'x'.repeat(80) })?.length, 48)
  assert.equal(doingOf('execute', { code: 'anything' }), undefined)
})

test('a report says what codn reads, and nothing that steers a terminal', () => {
  const record = oscOf({ state: 'working', id: 'ses a/b', title: 'a\x1b[31m\nb' })
  assert.equal(record.slice(1).includes('\x1b]'), false)
  assert.deepEqual(fieldsOf(record), { state: 'working', app: 'opencode', id: 'ses-a-b', title: 'a [31m b' })
  assert.equal(recordIdOf('x'.repeat(40)).length, 32)
  // A title is cut at 192 bytes, not in a character.
  const title = fieldsOf(oscOf({ state: 'working', id: 'a', title: 'a' + '€'.repeat(70) })).title
  assert.ok(Buffer.byteLength(title) <= 192 && !title.includes('�'), title)
  assert.equal(named('a'.repeat(60))?.length, 48)
  assert.equal(named('a.b.c.d.e.f.g.h.i.j'), undefined)
  assert.ok(isAnswer(QUESTION) && isAnswer('\x1b]7501;?\x07') && !isAnswer('\x1b]7501;state=idle\x1b\\'))
})
