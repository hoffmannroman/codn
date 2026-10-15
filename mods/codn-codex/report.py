#!/usr/bin/env python3
# What codn shows in the bar along its bottom edge, for a session of Codex:
# written to the file codn named to this start, when a turn has ended. The
# format is codn's, a few keys of TOML; the README of codn has them.
#
# Codex starts this as its `notify`, with what it has to say as the last
# argument. The file comes as the argument before that, not from the
# environment: Codex may run a turn in a server that many sessions share,
# and that server has the environment of whichever session started it.

import contextlib
import fcntl
import glob
import json
import os
import re
import stat
import sys
import tempfile
import time

# Who sits at the turn that ended. `codex exec`, started by something in a
# session, names itself otherwise, and must not put its figures over those
# of the program codn started. A name that is not here writes nothing.
ATTENDED = {'codex-tui'}

THREAD = re.compile(r'[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}')

# What codn takes at most: for a part in percent, for a count of tokens, of
# a name, and of limits. codn reads no report that could nest deeper than
# it can follow, and counts for that every dot, brace and bracket, also
# those in a name: a name with more of them than this is none, so that
# what else there is to say is still read.
MOST_USED = 999
MOST_TOKENS = 999e6
MOST_NAME = 48
MOST_LIMITS = 4
MOST_OPENERS = 8

# How much of a transcript is read at a time, from its end, and how far
# back at most. What a turn ran with stands at its beginning, and a turn
# can be long: one of 31 MiB was seen.
BLOCK = 1024 * 1024
MOST_READ = 256 * BLOCK
# How long the line may be that a transcript begins with.
MOST_FIRST = 16 * BLOCK
# How long a line may be that is read back. The lines that say what the
# bar shows are short, and one that carries what a tool printed is as long
# as that pleases.
MOST_LINE = 4 * BLOCK

# The lines that say what the bar shows. A line is looked into only if it
# names one of them.
SAID = (b'"token_count"', b'"turn_context"', b'"thread_settings_applied"')

# How long a report waits for another one that is being written, in seconds.
WAIT = 5.0


def table(value):
    return value if isinstance(value, dict) else {}


def parsed(text):
    """A line or an argument as the table it is. What is no JSON, no table
    or nested deeper than can be followed says nothing."""
    try:
        return table(json.loads(text))
    except (ValueError, RecursionError):
        return {}


def shown(value):
    """A number that can be shown: finite and not below zero."""
    is_number = isinstance(value, (int, float)) and not isinstance(value, bool)
    return is_number and value == value and 0 <= value < float('inf')


def named(text):
    """A name as codn shows it: nothing that steers a terminal, no half of
    a character, and no longer than codn takes."""
    if not isinstance(text, str):
        return None
    name = re.sub(r'[\x00-\x1f\x7f-\x9f\ud800-\udfff]', '', text).strip()[:MOST_NAME]
    return name if name and sum(name.count(opener) for opener in '.{[') <= MOST_OPENERS else None


def quoted(name):
    """A name as TOML takes it between quotes."""
    return json.dumps(name, ensure_ascii=False)


def ended(said):
    """The thread whose turn ended and the turn, if a person sits at it."""
    event = parsed(said)
    thread, turn, client = (event.get(key) for key in ('thread-id', 'turn-id', 'client'))
    is_attended = isinstance(client, str) and client in ATTENDED
    if event.get('type') != 'agent-turn-complete' or not is_attended:
        return None
    if not isinstance(thread, str) or not THREAD.fullmatch(thread):
        return None
    return thread, turn if isinstance(turn, str) and turn else None


def transcripts_of(thread):
    """The files Codex keeps of this thread, found by its id alone. Mostly
    one. After a prompt was taken back Codex goes on in a new file, named
    by the thread and a second id, and keeps the old one. Of the turns it
    runs for itself in a session, to give it a title, it keeps none."""
    home = os.environ.get('CODEX_HOME') or os.path.expanduser('~/.codex')
    folders = os.path.join(glob.escape(home), 'sessions', '*', '*', '*')
    names = (f'rollout-*-{thread}.jsonl', f'rollout-*-{thread}_*.jsonl')
    return [path for name in names for path in glob.glob(os.path.join(folders, name))]


def is_session(first, thread):
    """Whether a transcript begins as that of a session somebody started:
    under this id, and not as an agent another agent sent off."""
    line = parsed(first)
    meta = table(line.get('payload'))
    by_a_person = isinstance(meta.get('source'), str) and meta.get('thread_source', 'user') == 'user'
    return line.get('type') == 'session_meta' and meta.get('id') == thread and by_a_person


def opened(path, thread):
    """A transcript, open, if it is a file and that of the session. It is
    opened without waiting and asked afterwards what it is: a name can be
    made to stand for a pipe that nobody writes to."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    except OSError:
        return None
    if not stat.S_ISREG(os.fstat(descriptor).st_mode):
        os.close(descriptor)
        return None
    file = os.fdopen(descriptor, 'rb')
    if is_session(file.readline(MOST_FIRST), thread):
        return file
    file.close()
    return None


def lines_from_end(file, size):
    """The lines of a file, the last one first, as far back as is read. A
    line longer than is kept is passed over, and no more of it is held
    than that: put together and read as JSON it would take three times
    its length in memory."""
    # The pieces of a line whose beginning is not read yet, the last first,
    # and how long they are together. Of a line that is too long, none.
    pieces, held, at = [], 0, size
    while at > max(0, size - MOST_READ):
        step = min(BLOCK, at)
        at -= step
        file.seek(at)
        first, *lines = file.read(step).split(b'\n')
        if lines:
            beginning = lines.pop()
            if held + len(beginning) <= MOST_LINE:
                yield beginning + b''.join(reversed(pieces))
            pieces, held = [], 0
            yield from reversed(lines)
        held += len(first)
        if held <= MOST_LINE:
            pieces.append(first)
        else:
            pieces = []
    if at == 0 and held <= MOST_LINE:
        yield b''.join(reversed(pieces))


def count_line(key, value, most):
    """A whole number of tokens, or of seconds."""
    return [f'{key} = {round(value)}'] if shown(value) and value <= most else []


def runs_of(model, effort):
    """A model and how hard it thinks, if the line names a model."""
    model, effort = named(model), named(effort)
    if model is None:
        return None
    return [f'model = {quoted(model)}', *([f'options = [{quoted(effort)}]'] if effort else [])]


def tokens_of(count):
    """What the context holds and can hold. What the last call of the
    model held, not the sum of all calls."""
    return [
        *count_line('tokens', table(count.get('last_token_usage')).get('total_tokens'), MOST_TOKENS),
        *count_line('window', count.get('model_context_window'), MOST_TOKENS),
    ]


def span(minutes):
    """The name a limit is shown under: how long its window is."""
    if not shown(minutes) or minutes < 1 or minutes != int(minutes):
        return None
    minutes = int(minutes)
    for size, unit in ((24 * 60, 'd'), (60, 'h')):
        if minutes % size == 0:
            return f'{minutes // size}{unit}'
    return f'{minutes}m'


def limits_of(limits):
    """The limits by the name each is shown under. Of two by one name the
    first stands: TOML takes no table twice."""
    tables = {}
    for key, limit in limits.items():
        limit = table(limit)
        used = limit.get('used_percent')
        name = named(span(limit.get('window_minutes')) or key)
        if name is None or name in tables or not shown(used) or len(tables) == MOST_LIMITS:
            continue
        used = min(used, MOST_USED)
        tables[name] = [
            '',
            f'[limits.{quoted(name)}]',
            # As exact as it was said: codn rounds it, and only once.
            f'used = {int(used) if used == int(used) else float(used)!r}',
            *count_line('resets', limit.get('resets_at'), sys.maxsize),
        ]
    return [line for lines in tables.values() for line in lines]


def figures_of(file, size, turn=None):
    """What the transcript says last: what the session runs with, the
    tokens in the context, the limits, each from the last line that has
    it to say. Its format is Codex's own and may change, so keys are
    looked for, and what does not fit is left out. With a turn, also
    whether the transcript holds that turn."""
    runs = tokens = limits = None
    holds = turn is None
    mark = None if holds else json.dumps(turn).encode()
    for raw in lines_from_end(file, size):
        if not any(name in raw for name in SAID) and (holds or mark not in raw):
            continue
        line = parsed(raw)
        said = table(line.get('payload'))
        if line.get('type') not in ('turn_context', 'event_msg'):
            continue
        holds = holds or said.get('turn_id') == turn
        if line.get('type') == 'turn_context':
            runs = runs or runs_of(said.get('model'), said.get('effort'))
        elif said.get('type') == 'thread_settings_applied':
            settings = table(said.get('thread_settings'))
            runs = runs or runs_of(settings.get('model'), settings.get('reasoning_effort'))
        elif said.get('type') == 'token_count':
            tokens = tokens or tokens_of(table(said.get('info')))
            limits = limits or limits_of(table(said.get('rate_limits')))
        if runs and tokens and limits and holds:
            break
    return holds, [*(runs or []), *(tokens or []), *(limits or [])]


def report_of(thread, turn):
    """The report for a turn that ended, or nothing: what the bar shows
    then stays."""
    files = [file for file in (opened(path, thread) for path in transcripts_of(thread)) if file]
    try:
        sizes = {file: os.fstat(file.fileno()) for file in files}
        lines = []
        if len(files) == 1:
            _, lines = figures_of(files[0], sizes[files[0]].st_size)
        elif turn is not None:
            # Of several, the one that holds the turn. The one written
            # last is only where to look first, never the answer.
            for file in sorted(files, key=lambda file: sizes[file].st_mtime, reverse=True):
                holds, lines = figures_of(file, sizes[file].st_size, turn)
                if holds:
                    break
                lines = []
    finally:
        for file in files:
            file.close()
    return '\n'.join([*lines, '']) if lines else None


@contextlib.contextmanager
def alone(folder):
    """Holds the folder of the reports while one is read and written, and
    says whether it does. Two turns can end shortly after one another,
    and the report read first must not be the one written last."""
    try:
        held = os.open(folder, os.O_RDONLY | os.O_DIRECTORY)
    except OSError:
        # The folder is codn's to make; where it is gone, codn is too.
        yield False
        return
    try:
        until = time.monotonic() + WAIT
        while True:
            try:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                if time.monotonic() >= until:
                    yield False
                    return
                time.sleep(0.01)
                continue
            except OSError:
                # A file system that cannot do this: better a report.
                pass
            yield True
            return
    finally:
        os.close(held)


def write(status, text):
    """Puts the report in place in one move. codn must never read half of
    it: a number cut short reads as a smaller one."""
    folder, name = os.path.split(status)
    try:
        descriptor, beside = tempfile.mkstemp(dir=folder, prefix=f'{name}.', suffix='.tmp')
    except OSError:
        return
    try:
        with os.fdopen(descriptor, 'w', encoding='utf-8') as file:
            file.write(text)
        os.replace(beside, status)
    except OSError:
        with contextlib.suppress(OSError):
            os.unlink(beside)


def main(arguments):
    if len(arguments) != 2:
        return
    status, said = arguments
    turn_ended = ended(said)
    if not os.path.isabs(status) or turn_ended is None:
        return
    with alone(os.path.dirname(status)) as is_alone:
        text = report_of(*turn_ended) if is_alone else None
        if text is not None:
            write(status, text)


if __name__ == '__main__':
    # One that cannot be written must never get in the way of the session.
    try:
        main(sys.argv[1:])
    except Exception:
        pass
