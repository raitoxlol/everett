import sandbox  # noqa: F401

import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from everett import send
from everett.session import Session


class InstallationErrors(unittest.TestCase):
    def test_missing_harness_fails_before_waiting_for_a_session(self):
        session = Session('claude', 'busy', '', '/missing', '', 0)
        with mock.patch('everett.send.shutil.which', return_value=None), \
                mock.patch('everett.send.wait_idle') as wait:
            with self.assertRaisesRegex(send.SendError, 'not on PATH.*everett doctor'):
                send.send(session, 'review')
        wait.assert_not_called()

    def test_codex_output_is_removed_after_start_failure_or_timeout(self):
        for error in (FileNotFoundError('codex'), subprocess.TimeoutExpired('codex', 1)):
            with self.subTest(error=type(error).__name__), tempfile.TemporaryDirectory() as tmp:
                with mock.patch('everett.send.shutil.which', return_value='/bin/codex'), \
                        mock.patch('tempfile.tempdir', tmp), \
                        mock.patch('everett.send.subprocess.run', side_effect=error):
                    with self.assertRaises(send.SendError):
                        send.spawn('codex', 'review', tmp)
                self.assertEqual(list(Path(tmp).iterdir()), [])

    def test_invalid_request_does_not_allocate_a_codex_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch('tempfile.tempdir', tmp):
                with self.assertRaises(send.SendError):
                    send.spawn('codex', '', tmp)
            self.assertEqual(list(Path(tmp).iterdir()), [])
