"""Run the O-49 and O-64 checks individually on the ordinary Rust test thread stack."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

directory = Path(sys.argv[1])
records = map(json.loads, (directory / "pancakex-build.jsonl").read_text().splitlines())
binary, = (r["executable"] for r in records if r.get("executable") and r["profile"]["test"])
groups = {
    "venues::bsc_dex::pancakex::prior_tests::": 4,
    "venues::bsc_dex::pools::ambient_head::tests::registry_paths::": 2,
}
listed = subprocess.check_output([binary, "--list"], text=True, timeout=15).splitlines()
tests = []
for prefix, expected in groups.items():
    names = [line.removesuffix(": test") for line in listed
             if line.startswith(prefix) and line.endswith(": test")]
    if len(names) != expected:
        raise SystemExit(f"Expected {expected} tests under {prefix}, found {names}")
    tests.extend(names)
env = os.environ.copy()
env.pop("RUST_MIN_STACK", None)
report = {}
for name in tests:
    start = time.monotonic()
    with subprocess.Popen([binary, "--exact", name], env=env, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True) as test:
        try:
            output, _ = test.communicate(timeout=25)
            diagnostic = [v for v in output.splitlines() if re.search(
                r"test result:|panicked at|overflowed its stack|fatal runtime|assertion", v)][:8]
        except subprocess.TimeoutExpired:
            diagnostic = ["timeout"]
            if shutil.which("gdb"):
                try:
                    trace = subprocess.run(["sudo", "-n", "gdb", "-q", "--batch", "-ex",
                        "thread apply all bt 12", "-p", str(test.pid)], capture_output=True,
                        text=True, timeout=12, check=False)
                    diagnostic.extend(v[:400] for v in trace.stdout.splitlines()
                                      if re.match(r"Thread |#[0-9]+ ", v))
                except subprocess.TimeoutExpired:
                    diagnostic.append("debugger timeout")
            test.kill()
            test.communicate()
        report[name.rsplit("::", 1)[1]] = {
            "exit": test.returncode, "elapsed_ms": round((time.monotonic() - start) * 1000),
            "diagnostic": diagnostic[:80],
        }
    (directory / "pancakex-recovery.json").write_text(json.dumps(report, indent=2) + "\n")
if any(r["exit"] for r in report.values()):
    raise SystemExit("BSC isolated regression failed; see the bounded diagnostic artifact")
