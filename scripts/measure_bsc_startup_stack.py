"""Measure BSC startup futures and repeat the original regression on the runner's standard test stack."""
import argparse
import json
import os
import subprocess
import re
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("output", type=Path)
p.add_argument("--candidate", action="store_true")
p.add_argument("--load", action="store_true", help="measure the recorded parallel pool load")
args = p.parse_args()
target, size_test, regression = (
    ("bsc_dex_review_t153", "batch_load_future_sizes", "parallel_read_equals_sequential_on_recorded_answers")
    if args.load else
    ("bsc_dex_check_start", "bsc_startup_future_sizes", "invalid_during_pass_is_checked_as_soon_as_read")
)
compile_test = subprocess.run(["cargo","test","--locked","--test",target,"--no-run","--message-format=json-render-diagnostics"],
                              stdout=subprocess.PIPE, text=True, check=True)
exe, = [j["executable"] for l in compile_test.stdout.splitlines()
        if (j := json.loads(l)).get("executable") and j["target"]["name"]==target]
sizes_file = args.output.with_suffix(".sizes")
measure_env = {**os.environ,"ARB_CHECK_SIZES":str(sizes_file)}
if not args.candidate:
    measure_env["ARB_HISTORICAL_CHECK_SIZES"]="1"
subprocess.run([exe,"--exact",size_test],env=measure_env,check=True)
result = {"sha":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"sizes":json.loads(sizes_file.read_bytes()),"runs":[]}
# Linux x86_64: record the actual default-stack frames emitted for polling and constructing
# the relevant futures. Keeping the prologue lets the reader verify any stack-probe loops.
if args.load:
    nm = subprocess.check_output(["nm","-SC","--defined-only",exe],text=True)
    prefixes = ("arb::venues::bsc_dex::pools::load::state", "arb::venues::bsc_dex::pools::load::read",
                "arb::venues::bsc_dex::pools::load_batch::states", "arb::venues::bsc_dex::pools::load_site::states",
                "arb::venues::bsc_dex::pools::load_batch_par::states",
                "bsc_dex_review_t153::parallel_read_equals_sequential_on_recorded_answers")
    frames = []
    for line in nm.splitlines():
        fields = line.split(maxsplit=3)
        if len(fields)!=4 or fields[2] not in ("t","T"):
            continue
        address, size, _, symbol = fields
        # Legacy and v0 Rust symbols (GNU binutils may put a crate disambiguator inside []).
        name = re.sub(r"\[[0-9a-f]+\]", "",symbol)
        if not name.startswith(prefixes) or name.count("closure") > 1:
            continue
        start=int(address,16)
        dump=subprocess.check_output(["objdump","-d","-M","intel","--no-show-raw-insn",
              f"--start-address={start}",f"--stop-address={start+min(int(size,16),192)}",exe],text=True)
        prologue=[m[1] for row in dump.splitlines() if (m:=re.match(r"^\s*[0-9a-f]+:\s+(.+)$",row))]
        frames.append(dict(symbol=name,bytes=int(size,16),prologue=prologue[:22]))
    if not frames:
        raise ValueError("could not find BSC pool loader symbols")
    result["elf_x86_64"] = frames
normal_env=os.environ.copy()
normal_env.pop("RUST_MIN_STACK",None)
for _ in range(20):
    r=subprocess.run([exe,"--exact",regression],text=True,
                     env=normal_env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    if r.returncode==0 and "1 passed" not in r.stdout:
        raise ValueError("selected regression test did not run")
    result["runs"].append({"code":r.returncode,"overflow":"overflowed its stack" in r.stderr})
args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
if args.candidate and any(r["code"] for r in result["runs"]):
    raise SystemExit("BSC regression on the runner's default stack")
