"""Сравнение собственных выплат Goblins Router02, V5 и защищённым модулем на Base."""
import argparse
import json
from pathlib import Path

from fork_fee_rpc import Reader
from eth_abi import decode
from record_llama_base_cl import CanonicalCalls, EXEC, OWNER, ROOT, STEP, WETH, approval, balance, registry, tx, wallet

USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
DEGEN = "0x4ed4e862860bed51a9570b96d89af5e1b0efefed"
AERO = "0x940181a94a35a4569e4529a3cdfb74e38fd98631"


def degen_funding(book):
    def source(a,b):
        return next(m["pool"] for m in book["markets"] if set(m["tokens"]) == {a,b} and m["key"] == 10000)

    first, second = source(WETH,USDC), source(USDC,DEGEN)
    def fund(_cap,run,seed,deadline):
        prefix = [tx(WETH,"deposit()",value=seed),approval(WETH,EXEC,seed)]
        steps = [(1,first,WETH,USDC,10000,b""),(1,second,USDC,DEGEN,10000,b"")]
        swap = tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[WETH,seed,DEGEN,1,deadline,steps])
        probes = [balance(t,holder) for holder in (OWNER,EXEC) for t in (WETH,USDC,DEGEN)]
        rows=run(prefix+probes+[swap]+probes)
        assert all(r["status"]=="0x1" for r in rows)
        last=2+len(probes)
        before=[int(r["returnData"],16) for r in rows[2:last]]
        after=[int(r["returnData"],16) for r in rows[last+1:]]
        received=after[2]-before[2]
        assert before[0]-after[0]==seed and received>0 and after[1]==before[1] and after[3:]==before[3:]
        assert decode(["uint256"],bytes.fromhex(rows[last]["returnData"][2:]))==(received,)
        return prefix+[swap],received,dict(pools=[first,second],spent=str(seed),received=str(received),gas=int(rows[last]["gasUsed"],16))
    return fund


def capture(artifact, cash):
    snapshot = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    reader = Reader("base", snapshot)
    cap = CanonicalCalls(reader)
    book = registry(reader, cap, "goblins", snapshot)
    options = {}
    if cash:
        targets=[(m,DEGEN) for m in book["markets"] if set(m["tokens"])=={AERO,DEGEN} and m["liquidity"]>0]
        assert targets
        options=dict(selected_targets=targets,funding_source=degen_funding(book))
    native = wallet(reader, cap, book.copy(), **options)
    protected = wallet(reader, cap, book.copy(), artifact, **options)
    native["cases"].extend(protected["cases"])
    native["unavailable"].extend(protected["unavailable"])
    native["module_hash"] = protected["module_hash"]
    return native


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("--cash", action="store_true")
    args = parser.parse_args()
    args.output.write_text(json.dumps(capture(json.loads(args.artifact.read_bytes()), args.cash), separators=(",", ":")) + "\n", encoding="utf8")
