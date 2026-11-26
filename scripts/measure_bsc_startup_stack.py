"""Measure BSC startup futures and repeat the original regression on the runner's standard test stack."""
import argparse
import json
import os
import subprocess
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("output", type=Path)
p.add_argument("--candidate", action="store_true")
args = p.parse_args()
target = "bsc_dex_check_start"
compile_test = subprocess.run(["cargo","test","--locked","--test",target,"--no-run","--message-format=json-render-diagnostics"],
                              stdout=subprocess.PIPE, text=True, check=True)
exe, = [j["executable"] for l in compile_test.stdout.splitlines()
        if (j := json.loads(l)).get("executable") and j["target"]["name"]==target]
sizes_file = args.output.with_suffix(".sizes")
measure_env = {**os.environ,"ARB_CHECK_SIZES":str(sizes_file)}
subprocess.run([exe,"--exact","bsc_startup_future_sizes"],env=measure_env,check=True)
result = {"sha":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"sizes":json.loads(sizes_file.read_bytes()),"runs":[]}
normal_env=os.environ.copy()
normal_env.pop("RUST_MIN_STACK",None)
for _ in range(20):
    r=subprocess.run([exe,"--exact","invalid_during_pass_is_checked_as_soon_as_read"],text=True,
                     env=normal_env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    if r.returncode==0 and "1 passed" not in r.stdout:
        raise ValueError("startup regression test did not run")
    result["runs"].append({"code":r.returncode,"overflow":"overflowed its stack" in r.stderr})
args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
if args.candidate and any(r["code"] for r in result["runs"]):
    raise SystemExit("BSC startup regression on the runner's default stack")
