import fcntl
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import tracemalloc
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import report  # noqa: E402

THREAD = '01a11200-0000-7000-8000-000000000001'
OTHER = '01a11200-0000-7000-8000-000000000002'
TURN_ID = '01a11200-0000-7000-8000-000000000003'

# What the reporter writes for a turn of TURN. src/core/reported.rs of codn
# reads the same file in a test of its own.
REPORT = (HERE / 'report.toml').read_text()
# The same without what the session runs with, and that alone.
FIGURES = REPORT.split('\n', 2)[2]
RUNS = 'model = "gpt-6.1-sol"\noptions = ["high"]\n'


def meta(thread=THREAD, **more):
    return {'type': 'session_meta', 'payload': {'id': thread, 'source': 'cli', 'thread_source': 'user', **more}}


def context(model='gpt-6.1-sol', effort='high', turn=TURN_ID):
    return {'type': 'turn_context', 'payload': {'turn_id': turn, 'model': model, 'effort': effort, 'cwd': '/work'}}


def settings(model, effort):
    said = {'type': 'thread_settings_applied', 'thread_settings': {'model': model, 'reasoning_effort': effort}}
    return {'type': 'event_msg', 'payload': said}


def limit(used, minutes, resets=None):
    return {'used_percent': used, 'window_minutes': minutes, **({} if resets is None else {'resets_at': resets})}


LIMITS = {
    'limit_id': 'codex',
    'primary': limit(33.0, 300, 1791310998),
    'secondary': limit(18.0, 10080, 1791833854),
    'credits': {'has_credits': False, 'balance': '0'},
    'individual_limit': None,
}
# What Codex says of an account whose limits it does not know just then.
NO_LIMITS = {'limit_id': 'premium', 'primary': None, 'secondary': None, 'credits': {'has_credits': True}}


def count(last=21382, window=760000, total=42782, limits=LIMITS, kind='event_msg'):
    info = {
        'total_token_usage': {'total_tokens': total},
        'last_token_usage': {'total_tokens': last},
        'model_context_window': window,
    }
    return {'type': kind, 'payload': {'type': 'token_count', 'info': info, 'rate_limits': limits}}


def message(text):
    return {'type': 'response_item', 'payload': {'type': 'message', 'content': [{'text': text}]}}


def done(turn=TURN_ID):
    return {'type': 'event_msg', 'payload': {'type': 'task_complete', 'turn_id': turn, 'last_agent_message': 'ok'}}


TURN = [meta(), context(), message('ok'), count(), done()]


def ended(thread=THREAD, client='codex-tui', turn=TURN_ID, **more):
    said = {'type': 'agent-turn-complete', 'thread-id': thread, 'cwd': '/work', **more}
    said = said if turn is None else {**said, 'turn-id': turn}
    return json.dumps(said if client is None else {**said, 'client': client})


class Reporting(unittest.TestCase):
    """A home of Codex with transcripts in it, and the folder codn made for
    the files it names."""

    def setUp(self):
        self.root = pathlib.Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.home = self.root / 'codex'
        self.status = self.root / 'status' / '5-1.toml'
        self.status.parent.mkdir()
        self.enterContext(mock.patch.dict(os.environ, {'CODEX_HOME': str(self.home)}))

    def transcript(self, lines, thread=THREAD, day='06', tail=b'', second=''):
        folder = self.home / 'sessions' / '2026' / '10' / day
        folder.mkdir(parents=True, exist_ok=True)
        text = b''.join(json.dumps(line).encode() + b'\n' for line in lines)
        path = folder / f'rollout-2026-10-{day}T19-43-13-{thread}{second}.jsonl'
        path.write_bytes(text + tail)
        return path

    def reported(self, said=None, status=None):
        report.main([str(status or self.status), ended() if said is None else said])
        return self.status.read_text() if self.status.exists() else None

    def left(self):
        return sorted(path.name for path in self.status.parent.iterdir())


class TestReport(Reporting):
    def test_a_turn_that_ended_is_written_as_the_report_codn_reads(self):
        self.transcript(TURN)

        self.assertEqual(self.reported(), REPORT)
        self.assertEqual(self.left(), ['5-1.toml'], 'nothing is left beside the report')
        self.assertEqual(self.status.stat().st_mode & 0o777, 0o600)

    def test_the_file_is_the_one_in_the_arguments_not_that_of_the_environment(self):
        self.transcript(TURN)
        inherited = self.root / 'status' / '4-1.toml'

        with mock.patch.dict(os.environ, {'CODN_STATUS': str(inherited)}):
            report.main([ended()])
            self.assertEqual(self.left(), [], 'a file that was not named is not written')
            self.assertEqual(self.reported(), REPORT)

        self.assertEqual(self.left(), ['5-1.toml'])

    def test_a_file_that_is_no_whole_path_is_not_written(self):
        self.transcript(TURN)

        # Codex starts its notify in a folder of its own choosing.
        self.addCleanup(os.chdir, os.getcwd())
        os.chdir(self.root)
        for status in ('', '{status}', '5-1.toml', 'status/5-1.toml', './status/5-1.toml'):
            report.main([status, ended()])
        report.main([str(self.status), ended(), 'more'])

        self.assertEqual(self.left(), [])

    def test_a_turn_nobody_sits_at_writes_nothing(self):
        self.transcript(TURN)

        for client in ('codex_exec', 'codex-tui-2', '', None):
            self.assertIsNone(self.reported(ended(client=client)), client)
        self.assertIsNone(self.reported(ended().replace('agent-turn-complete', 'approval-requested')))
        # Nor does what is no such event, whatever it is instead.
        for said in ('', 'ok', '[]', '{"type": "agent-turn-complete"', ended().replace('"codex-tui"', '["codex-tui"]')):
            self.assertIsNone(self.reported(said), said)
        self.assertIsNone(self.reported('[' * 100_000))

    def test_a_turn_of_codex_itself_leaves_the_report_as_it_is(self):
        # The title of a conversation is made in a thread of its own, at
        # the same client and with no transcript, right after the first turn.
        self.transcript(TURN)
        self.assertEqual(self.reported(), REPORT)

        self.assertEqual(self.reported(ended(OTHER)), REPORT)

    def test_a_thread_is_named_by_an_id_and_by_nothing_else(self):
        # Names that stand for more than one file, each in a transcript
        # that agrees with it.
        for thread in ('*', THREAD + '*', THREAD[:-1] + '?'):
            with self.subTest(thread):
                self.transcript([meta(thread), *TURN[1:]])
                self.assertIsNone(self.reported(ended(thread)))

    def test_only_the_session_somebody_started_reports(self):
        sent_off = {'subagent': {'thread_spawn': {'parent_thread_id': THREAD}}}
        for first in (
            meta(source=sent_off),
            meta(thread_source='subagent'),
            meta(thread_source='memory_consolidation'),
            meta(OTHER),
            {**meta(), 'type': 'event_msg'},
            'session_meta',
        ):
            self.transcript([first, *TURN[1:]])
            self.assertIsNone(self.reported(), first)
        # What older versions did not say is not held against a session.
        self.transcript([{'type': 'session_meta', 'payload': {'id': THREAD, 'source': 'vscode'}}, *TURN[1:]])
        self.assertEqual(self.reported(), REPORT)

    def test_what_stands_where_a_transcript_belongs_is_not_read(self):
        folder = self.home / 'sessions' / '2026' / '10' / '06'
        (folder / f'rollout-2026-10-06T19-43-13-{THREAD}.jsonl').mkdir(parents=True)
        self.assertIsNone(self.reported())

        # A pipe that nobody writes to must not keep the reporter waiting.
        os.mkfifo(folder / f'rollout-2026-10-07T00-00-00-{THREAD}_{OTHER}.jsonl')
        program = [sys.executable, str(HERE.parent / 'report.py'), str(self.status), ended()]
        subprocess.run(program, capture_output=True, timeout=5)
        self.assertEqual(self.left(), [])

    def test_after_a_prompt_was_taken_back_the_new_transcript_is_read(self):
        # Codex goes on in a second file and keeps the first, with all the
        # turns that were taken back.
        old = self.transcript(TURN)
        again = '01a11300-0000-7000-8000-000000000001'
        new = [meta(), context('gpt-6-luna', 'low', again), count(last=900, total=900), done(again)]
        self.transcript(new, day='07', second=f'_{OTHER}')
        after = 'model = "gpt-6-luna"\noptions = ["low"]\ntokens = 900\n'

        self.assertTrue(self.reported(ended(turn=again)).startswith(after))
        # Not by which was written last: the turn is what tells them apart.
        os.utime(old, ns=(2**62, 2**62))
        self.assertTrue(self.reported(ended(turn=again)).startswith(after))
        self.assertEqual(self.reported(ended(turn=TURN_ID)), REPORT)

        # A turn that neither holds, or none at all, says nothing.
        self.status.unlink()
        for turn in (OTHER, '', None):
            self.assertIsNone(self.reported(ended(turn=turn)), turn)
        # Alone, the second file is the transcript.
        old.unlink()
        self.assertTrue(self.reported(ended(turn=None)).startswith(after))

    def test_what_is_said_last_is_what_is_shown(self):
        # The sum of all calls is not what the context holds, and after a
        # compaction it holds less than before.
        lines = [*TURN, context('gpt-6.1-sol', 'low'), count(last=30000, total=90000), done()]
        self.transcript([*lines, count(last=5815, total=90000)])
        text = self.reported()
        self.assertIn('options = ["low"]\ntokens = 5815\nwindow = 760000\n', text)

        # A model chosen after the turn is the one the next turn runs with.
        self.transcript([*lines, settings('gpt-6-luna', 'medium')])
        self.assertIn('model = "gpt-6-luna"\noptions = ["medium"]\ntokens = 30000\n', self.reported())

        # A line that names no model does not hide the one before it.
        self.transcript([*lines, settings(None, 'max'), {'type': 'turn_context', 'payload': {}}])
        self.assertIn('model = "gpt-6.1-sol"\noptions = ["low"]\n', self.reported())

    def test_a_line_with_nothing_to_show_does_not_hide_the_one_before_it(self):
        # Limits that Codex did not know for a moment, a count without
        # figures, a model that is no name.
        unknown = count(limits=NO_LIMITS)
        unknown['payload']['info'] = {'last_token_usage': {}, 'model_context_window': None}
        nothing = {'type': 'event_msg', 'payload': {'type': 'token_count', 'info': None, 'rate_limits': None}}
        self.transcript([*TURN, unknown, nothing, context('\x00\x1b', 'max'), done()])

        self.assertEqual(self.reported(), REPORT)

    def test_figures_come_from_the_lines_that_have_them(self):
        only_limits = {'type': 'event_msg', 'payload': {'type': 'token_count', 'info': None, 'rate_limits': LIMITS}}
        only_tokens = count(last=7, window=100, limits=None)
        self.transcript([meta(), context(effort=None), only_tokens, only_limits, done()])

        text = self.reported()

        self.assertTrue(text.startswith('model = "gpt-6.1-sol"\ntokens = 7\nwindow = 100\n\n[limits."5h"]\n'), text)

    def test_a_limit_is_named_by_how_long_its_window_is(self):
        limits = {
            'primary': limit(12.50001, 300),
            'secondary': limit(999.5, 3 * 24 * 60, 1791833854.4),
            'again': limit(3, 300),
            'none': limit(float('nan'), 60),
            'less': limit(-1, 120),
            'yes': limit(True, 180),
            'a.b.c.d.e.f.g.h.i.j': limit(6, None),
            'odd "name"': limit(4, 90.5, 1e30),
            'monthly': limit(1, 90),
            'spark': limit(2, None),
        }
        self.transcript([meta(), count(limits=limits)])

        text = self.reported()

        # src/core/reported.rs of codn reads this text too: change both
        # together. No more limits are written than codn shows.
        self.assertEqual(
            text.split('\n\n', 1)[1],
            '[limits."5h"]\nused = 12.50001\n\n[limits."3d"]\nused = 999\nresets = 1791833854\n\n'
            '[limits."odd \\"name\\""]\nused = 4\n\n[limits."90m"]\nused = 1\n',
        )

    def test_what_cannot_be_shown_is_left_out(self):
        odd = count(last=1e12, window=float('inf'), limits='none')
        self.transcript([meta(), context('gpt "6"\x1b[31m\n\x7f\x85\ud800', 'hi\x00gh'), odd])
        said = 'model = "gpt \\"6\\"[31m"\noptions = ["high"]\n'
        self.assertEqual(self.reported(), said)

        self.transcript([meta(), context(model=7, effort=['high']), count(last=-5, window='wide', limits={})])
        self.assertEqual(self.reported(), said, 'what says nothing does not take the place of what was said')

        # A window is bounded like a count, and a name like codn bounds it.
        self.transcript([meta(), context('m' * 60, 'e' * 60), count(last=5, window=1e12, limits=None)])
        self.assertEqual(self.reported(), f'model = "{"m" * 48}"\noptions = ["{"e" * 48}"]\ntokens = 5\n')

    def test_a_name_codn_would_not_read_does_not_cost_the_figures(self):
        # codn counts dots, braces and brackets through the whole report.
        self.transcript([meta(), context('[' * 40, 'a.b'), count(), done()])
        self.assertEqual(self.reported(), FIGURES)

        self.transcript([meta(), context('gpt-6.1-sol', '{[.' * 4), count(), done()])
        self.assertEqual(self.reported(), 'model = "gpt-6.1-sol"\n' + FIGURES)

    def test_a_transcript_is_not_taken_at_its_word(self):
        # Codex is in the middle of a line, a line is no JSON or nests
        # deeper than can be followed, a message speaks of the keys that
        # are looked for, a number stands where a table belongs.
        talk = {'type': 'response_item', 'payload': {'token_count': 1, 'turn_context': 2, 'model': 'said'}}
        quoted = count(last=1, limits={'primary': limit(1, 60)}, kind='response_item')
        junk = {'type': 'event_msg', 'payload': {'type': 'token_count', 'info': 3, 'rate_limits': [1]}}
        deep = b'{"type":"event_msg","payload":{"type":"token_count","info":' + b'[' * 100_000 + b'\n'
        half = b'{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_to'
        self.transcript([*TURN, talk, quoted, junk, 5, 'token_count'], tail=deep + half)

        self.assertEqual(self.reported(), REPORT)

    def test_what_a_turn_ran_with_is_found_far_behind_its_end(self):
        printed = [message('x' * 700) for _ in range(40)]
        with mock.patch.object(report, 'BLOCK', 1024), mock.patch.object(report, 'MOST_READ', 64 * 1024):
            # Also where its line is longer than what is read at a time.
            long = context()
            long['payload']['developer_instructions'] = 'z' * 3000
            self.transcript([meta(), long, *printed, message('y' * 5000), count(), done()])
            self.assertEqual(self.reported(), REPORT)

            # Not further back than is read: the figures stand without it.
            self.transcript([meta(), context(), *printed * 3, count(), done()])
            self.assertEqual(self.reported(), FIGURES)

            # A session that has said nothing yet leaves the report.
            self.transcript([meta()])
            self.assertEqual(self.reported(), FIGURES)

    def test_a_line_longer_than_is_kept_is_passed_over(self):
        with mock.patch.object(report, 'BLOCK', 1024), mock.patch.object(report, 'MOST_LINE', 4096):
            # One that says with what the turn ran, and is kept to its end
            # by nothing: what the line before it said stands.
            long = context('gpt-6-luna', 'low')
            long['payload']['developer_instructions'] = 'z' * 5000
            self.transcript([*TURN, long, done()])
            self.assertEqual(self.reported(), REPORT)

            # Also at the end of the transcript, and behind its first line.
            self.transcript([*TURN, long])
            self.assertEqual(self.reported(), REPORT)
            self.transcript([meta(), long, count(), done()])
            self.assertEqual(self.reported(), FIGURES)

    def test_a_line_is_given_whole_or_not_at_all(self):
        # Lines longer than is kept at the beginning and in the middle, and
        # between them one that is as long as is kept.
        path = self.root / 'lines'
        path.write_bytes(b'z' * 5000 + b'\nfirst\n' + b'y' * 4096 + b'\n' + b'x' * 4097 + b'\nlast')

        with mock.patch.object(report, 'BLOCK', 1024), mock.patch.object(report, 'MOST_LINE', 4096):
            with open(path, 'rb') as file:
                lines = list(report.lines_from_end(file, path.stat().st_size))

        self.assertEqual(lines, [b'last', b'y' * 4096, b'first'])

    def test_no_more_of_a_long_line_is_held_than_is_kept(self):
        # A line of 4 MiB that says nothing, read back in pieces.
        self.transcript([*TURN, message('x' * (4 * 1024 * 1024))])
        most = 256 * 1024

        with mock.patch.object(report, 'BLOCK', 64 * 1024), mock.patch.object(report, 'MOST_LINE', most):
            tracemalloc.start()
            self.addCleanup(tracemalloc.stop)
            self.assertEqual(self.reported(), REPORT)
            _, peak = tracemalloc.get_traced_memory()

        self.assertLess(peak, 4 * most)

    def test_the_first_line_is_read_no_further_than_it_may_be_long(self):
        self.transcript([meta(base_instructions='z' * 200), *TURN[1:]])

        with mock.patch.object(report, 'MOST_FIRST', 100):
            self.assertIsNone(self.reported())
        self.assertEqual(self.reported(), REPORT)

    def test_the_home_of_codex_is_found_where_it_is(self):
        with mock.patch.dict(os.environ, {'HOME': str(self.root / 'h[o]me')}):
            # Where nothing says otherwise.
            self.home = self.root / 'h[o]me' / '.codex'
            self.transcript(TURN)
            del os.environ['CODEX_HOME']
            self.assertEqual(self.reported(), REPORT)

            # And in a folder whose name could be read as a pattern.
            self.status.unlink()
            self.home = self.root / 'c[o]dex'
            self.transcript([meta(), context('other')])
            os.environ['CODEX_HOME'] = str(self.home)
            self.assertEqual(self.reported(), 'model = "other"\noptions = ["high"]\n')

    def test_the_report_is_put_in_place_in_one_move(self):
        self.transcript(TURN)
        self.status.write_text('tokens = 1\n')
        before = self.root / 'status' / 'before'
        os.link(self.status, before)

        self.assertEqual(self.reported(), REPORT)

        # What was there was not written into: a reader that had it open
        # goes on reading what it was.
        self.assertEqual(before.read_text(), 'tokens = 1\n')
        self.assertEqual(self.left(), ['5-1.toml', 'before'])

    def test_a_report_waits_for_one_that_is_being_written(self):
        self.transcript(TURN)
        other = os.open(self.status.parent, os.O_RDONLY)
        self.addCleanup(os.close, other)
        fcntl.flock(other, fcntl.LOCK_EX)

        with mock.patch.object(report, 'WAIT', 0.05):
            self.assertIsNone(self.reported(), 'not around the one that holds the folder')
            fcntl.flock(other, fcntl.LOCK_UN)
            self.assertEqual(self.reported(), REPORT)

    def test_a_report_that_cannot_be_put_in_place_leaves_nothing_behind(self):
        self.transcript(TURN)
        gone = self.root / 'gone' / '5-1.toml'
        folder = self.root / 'status' / 'folder.toml'
        folder.mkdir()

        report.main([str(gone), ended()])
        report.main([str(folder), ended()])

        self.assertFalse(gone.parent.exists(), 'the folder is codn\'s to make')
        self.assertEqual(self.left(), ['folder.toml'])
        self.assertEqual(list(folder.iterdir()), [])

    def test_it_runs_as_the_program_codex_starts(self):
        self.transcript(TURN)
        # As Codex is told to start it: the file itself, by its first line.
        program = [str(HERE.parent / 'report.py')]

        ran = subprocess.run([*program, str(self.status), ended()], capture_output=True, timeout=20)

        self.assertEqual((ran.returncode, ran.stdout, ran.stderr), (0, b'', b''))
        self.assertEqual(self.status.read_text(), REPORT)
        # Whatever it is given, it ends quietly: Codex does not wait for it.
        for arguments in ([], [str(self.status)], [str(self.status), '\ud800'.encode('utf-8', 'surrogatepass')]):
            ran = subprocess.run([*program, *arguments], capture_output=True, timeout=20)
            self.assertEqual((ran.returncode, ran.stdout, ran.stderr), (0, b'', b''), arguments)
        # Also when something it did not foresee goes wrong: here, a
        # module of Python that is not what it expects.
        self.status.unlink()
        shadow = self.root / 'shadow'
        shadow.mkdir()
        (shadow / 'tempfile.py').write_text('def mkstemp(**named):\n    raise RuntimeError("unforeseen")\n')
        env = {**os.environ, 'PYTHONPATH': str(shadow)}
        ran = subprocess.run([*program, str(self.status), ended()], capture_output=True, timeout=20, env=env)
        self.assertEqual((ran.returncode, ran.stdout, ran.stderr), (0, b'', b''))
        self.assertEqual(self.left(), [], 'the module was the one that fails')


if __name__ == '__main__':
    unittest.main()
