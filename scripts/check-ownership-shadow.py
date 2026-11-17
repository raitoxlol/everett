#!/usr/bin/env python3
"""Score auth-backed ownership tests; any build/test failure scores one."""
import os
from pathlib import Path
import subprocess
root = Path(__file__).resolve().parents[1]
env = dict(os.environ, CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_TEST_DEBUG="0", CARGO_INCREMENTAL="0")
result = subprocess.run([str(Path.home()/".cargo/bin/cargo"), "test", "--locked", "--manifest-path", str(root/"everett-rs/Cargo.toml"), "--lib", "gateway::ownership_shadow"], env=env, capture_output=True, text=True)
print(result.stdout)
print(result.stderr)
print("failures=" + str(int(result.returncode != 0)))
