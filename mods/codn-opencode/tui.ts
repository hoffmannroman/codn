// The plugin of opencode's terminal interface through which a session of
// opencode in codn says what it does: the report in the file codn names in
// CODN_STATUS, the state of the session and of its agents in reports of
// OSC 7501, and in a file of its own which conversation it is in, so that
// codn resumes that one after a restart. What it says is made in
// `report.ts`. Outside codn the variable is not set and the plugin does
// nothing.

import { Plugin } from '@opencode/plugin/tui'
import { createEffect, createRoot, untrack } from 'solid-js'
import { mkdirSync, rmSync } from 'node:fs'
import { rename, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { isAnswer, knownOf, oscOf, QUESTION, reportOf, resumed, runningOf, show, take } from './report.ts'
import type { Model, Outcome } from './report.ts'

// Where the file lies that says which conversation the process is in, by
// its process id. codn's history of opencode names the same place.
const RUNNING = join(homedir(), '.local', 'state', 'codn-opencode')

// How many reports wait for the terminal's answer at the most.
const MOST_WAITING = 64

// A file written whole or not at all, one write at a time, and a text that
// was written last not again. One that cannot be written must never get in
// the way of the session: the next text that is made writes it.
const writerOf = (file: string) => {
  let [isWriting, due, written] = [false, undefined as string | undefined, undefined as string | undefined]
  const write = async (text: string): Promise<void> => {
    due = text
    if (isWriting) return

    isWriting = true
    try {
      while (due !== undefined) {
        const next = due
        due = undefined
        if (next === written) continue
        try {
          const part = `${file}.${process.pid}.part`
          await writeFile(part, next)
          await rename(part, file)
          written = next
        } catch {
          written = undefined
        }
      }
    } finally {
      isWriting = false
    }
  }
  return write
}

export default Plugin.define({
  id: 'codn.status',
  setup(context) {
    const status = process.env.CODN_STATUS
    if (status === undefined || status === '') return

    const known = knownOf()
    const writeReport = writerOf(status)
    const running = join(RUNNING, `${process.pid}.json`)
    const writeRunning = writerOf(running)
    try {
      // What a process that had this id before wrote is not this one's.
      mkdirSync(RUNNING, { recursive: true, mode: 0o700 })
      rmSync(running, { force: true })
    } catch {
      // Nothing to do: codn does not resume what it was not told of.
    }

    // The reports of OSC 7501 go to a terminal that said it takes them,
    // and through the renderer, which writes what it draws in pieces of
    // its own: a report written beside it could land in one of them.
    const renderer = context.renderer as unknown as {
      writeOut?: (text: string) => unknown
      subscribeOsc?: (listener: (sequence: string) => void) => () => void
    }
    let isAnswered = false
    const waiting: string[] = []
    const send = (records: string[]) => {
      if (!isAnswered) {
        waiting.push(...records)
        waiting.splice(0, Math.max(0, waiting.length - MOST_WAITING))
        return
      }
      for (const record of records) {
        try {
          renderer.writeOut?.(record)
        } catch {
          // Nothing to do: codn goes on guessing from the terminal.
        }
      }
    }
    const unsubscribe = renderer.subscribeOsc?.(sequence => {
      if (isAnswered || !isAnswer(sequence)) return
      isAnswered = true
      send(waiting.splice(0))
    })
    try {
      renderer.writeOut?.(QUESTION)
    } catch {
      // Nothing to do: without an answer no report is sent.
    }

    // How many tokens a model takes, as the list of models says.
    const location = context.location ?? context.data.location.default()
    let models: unknown[] = []
    let isAsking = false
    const ask = () => {
      if (isAsking) return
      isAsking = true
      void context.data.location.model
        .sync(location)
        .then(() => {
          models = (context.data.location.model.list(location) as unknown[] | undefined) ?? []
          if (known.model !== undefined && known.window === undefined) {
            known.window = windowOf(known.model)
            if (known.window !== undefined) did({ report: true, records: [] })
          }
        })
        .catch(() => undefined)
        .finally(() => {
          isAsking = false
        })
    }
    const windowOf = (model: Model): number | undefined => {
      const found = models.find((m): m is { limit?: { context?: unknown } } => {
        const each = m as { id?: unknown; modelID?: unknown; providerID?: unknown }
        return (each.id === model.id || each.modelID === model.id) && each.providerID === model.provider
      })
      const window = found?.limit?.context
      if (typeof window === 'number') return window
      ask()
      return undefined
    }

    const did = (outcome: Outcome) => {
      send(outcome.records)
      if (outcome.report) void writeReport(reportOf(known))
    }

    // The file that names the conversation, written again where its last
    // write failed: the writer passes over a text it has written.
    const tell = () => {
      const text = runningOf(known)
      if (text !== undefined) void writeRunning(text)
    }

    // The conversation the window shows. A person changes it with no event
    // of the server, so the route is followed as the interface follows it.
    const look = (route: { type?: string; sessionID?: string }) => {
      try {
        const session = route.type === 'session' ? route.sessionID : undefined
        if (session === known.shown) return

        const running = session !== undefined && context.data.session.status(session) === 'running'
        did(show(known, session, running))
        tell()
        if (session === undefined) return

        // What opencode kept of the conversation, which it does not hand
        // the plugin of one it did not see begin until it is asked.
        void (async () => {
          await context.data.session.sync(session)
          await context.data.session.message.sync(session)
          const messages = context.data.session.message.list(session)
          did(resumed(known, session, context.data.session.get(session), messages, windowOf))
        })().catch(() => undefined)
      } catch {
        // Nothing to do: the next change of the route looks again.
      }
    }

    ask()
    let unfollow = () => {}
    createRoot(dispose => {
      unfollow = dispose
      // Only the route is followed, not what is read to show it. The route
      // is a store, followed by the parts of it that are read: both are
      // read here, and not in what follows untracked.
      createEffect(() => {
        const route = context.ui.router.current() as { type?: string; sessionID?: string }
        const [type, sessionID] = [route.type, route.sessionID]
        untrack(() => look({ type, sessionID }))
      })
    })
    const stop = context.data.listen((event: { details?: { type?: unknown; created?: unknown; data?: unknown } }) => {
      try {
        tell()
        const details = event.details ?? {}
        if (typeof details.type !== 'string') return
        const created = typeof details.created === 'number' ? details.created : undefined
        const data = typeof details.data === 'object' && details.data !== null ? (details.data as Record<string, unknown>) : {}
        did(take(known, details.type, created, data, windowOf))
      } catch {
        // Nothing to do: what an event would have said, the next one says.
      }
    })

    return () => {
      stop()
      unfollow()
      // What was said goes with the program: its agents and its state.
      send([...known.agents.keys()].map(id => oscOf({ state: 'clear', id })))
      send([oscOf({ state: 'clear' })])
      unsubscribe?.()
    }
  },
})
