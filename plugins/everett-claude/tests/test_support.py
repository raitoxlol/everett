import os


BASE_ENV = {key: value for key, value in os.environ.items()
            if key != "CLAUDECODE" and not key.startswith("CLAUDE_CODE_")}
