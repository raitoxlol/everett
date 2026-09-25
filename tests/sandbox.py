"""Import first in every test module: points HOME at a throwaway dir for the whole run.

Tests must never read or write the real ~/.claude, ~/.codex, ~/.omp, ~/.hermes, ~/.pi, ~/.grok, ~/.t3
or ~/.everett.
"""
import atexit
import os
import shutil
import tempfile

HOME = tempfile.mkdtemp(prefix='everett-test-home-')
atexit.register(shutil.rmtree, HOME, True)
os.environ['HOME'] = HOME
os.environ['EVERETT_HOME'] = HOME
os.environ['EVERETT_NOTIFY'] = 'none'  # tests never raise real notifications
for name in ('TYPESAFE_API_KEY', 'EVERETT_VAULT', 'EVERETT_VAULT_DIR', 'EVERETT_ROUTER', 'EVERETT_HARNESS',
             'EVERETT_SEND', 'EVERETT_HOPS', 'EVERETT_SESSION_ID', 'EVERETT_MERGE_LLM', 'CLAUDE_CODE_ENTRYPOINT',
             'CLAUDECODE', 'CLAUDE_CODE_SESSION_ID', 'CLAUDE_SESSION_ID', 'CODEX_THREAD_ID', 'HERMES_SESSION_ID',
             'PI_SESSION_FILE', 'GROK_SESSION_ID', 'EVERETT_NOTIFY_COMMAND', 'EVERETT_ESCALATE_MINUTES'):
    os.environ.pop(name, None)
