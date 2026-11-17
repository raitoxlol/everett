#!/usr/bin/env python3
"""CLI acceptance score; a failed build or test command scores one."""
import os
from pathlib import Path
import subprocess
root=Path(__file__).resolve().parents[1]
env=dict(os.environ,CARGO_PROFILE_DEV_DEBUG="0",CARGO_PROFILE_TEST_DEBUG="0",CARGO_INCREMENTAL="0")
p=subprocess.run([str(Path.home()/".cargo/bin/cargo"),"test","--locked","--manifest-path",str(root/"everett-rs/Cargo.toml"),"--test","gateway_native","--","identity_diagnostic"],env=env,capture_output=True,text=True)
print(p.stdout);print(p.stderr);print("failures="+str(int(p.returncode != 0)))
