#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
r=Path(__file__).resolve().parents[1]
env=dict(os.environ,CARGO_PROFILE_DEV_DEBUG="0",CARGO_PROFILE_TEST_DEBUG="0",CARGO_INCREMENTAL="0")
p=subprocess.run([str(Path.home()/".cargo/bin/cargo"),"test","--locked","--manifest-path",str(r/"everett-rs/Cargo.toml"),"--test","gateway_native","--","whoami_provenance"],env=env,capture_output=True,text=True)
print(p.stdout);print(p.stderr);print("failures="+str(int(p.returncode != 0)))
