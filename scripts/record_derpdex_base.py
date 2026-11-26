"""Reproduce DerpDEX Base source differences, canonical factory membership and wallet settlement."""
import argparse
import hashlib
import json
from pathlib import Path
import posixpath
import re

from eth_utils import keccak
import requests
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, rpc
from record_pool_executor_controls import record
from record_rubicon_clmm import FOLDER, pool_address

POOL = "0x09b852108b6ed78adb2e776d901b8d54ba708080"
FACTORY = "0xeddef4273518b137cdbcb3a7fa1c6a688303dfe2"
REF_POOL = "0xb4cb800910b228ed3d0834cf79d697127bbb00e5"
REF_FACTORY = "0x33128a8fc17869897dce68ed026d694621f6fdfd"
WETH = "0x4200000000000000000000000000000000000006"
LENS = "0x0cdee061c75d43c82520ed998c23ac2991c9ac6d"


def proof(address, cap):
    url = f"https://sourcify.dev/server/v2/contract/8453/{address}"
    response = requests.get(url, params={"fields": "compilation,sources,runtimeBytecode.onchainBytecode,creationBytecode.onchainBytecode"}, timeout=30)
    response.raise_for_status()
    info = response.json()
    runtime = info["runtimeBytecode"]["onchainBytecode"]
    assert info["runtimeMatch"] and rpc(cap.endpoint,"eth_getCode", [address, cap.selector]).lower() == runtime.lower()
    pending = [info["compilation"]["fullyQualifiedName"].split(":")[0]]
    code = {}
    while pending:
        name = pending.pop()
        if name in code:
            continue
        body = re.sub(r"/\*.*?\*/|//[^\n]*", "", info["sources"][name]["content"], flags=re.S)
        pre = re.findall(r'\bimport\s+["\']([^"\']+)["\']\s*;', body)
        assert len(pre) == len(re.findall(r"\bimport\b", body))
        pending.extend(posixpath.normpath(posixpath.join(posixpath.dirname(name), f)) for f in pre)
        code[name] = info["sources"][name]["content"]
    creation = bytes.fromhex(info["creationBytecode"]["onchainBytecode"][2:])
    return {"url": url, "runtime_keccak": "0x"+keccak(bytes.fromhex(runtime[2:])).hex(),
            "init_hash": "0x"+keccak(creation).hex()}, code


def main(output):
    snapshot = json.loads((FOLDER/"base.json").read_bytes())
    c = Capture(snapshot)
    witness = {}
    for role, address, ref in [("pool", POOL, REF_POOL), ("factory", FACTORY, REF_FACTORY)]:
        actual, code = proof(address, c)
        original, baseline = proof(ref, c)
        assert set(baseline) == set(code)
        # Only differences allowed across the complete reachable import graph.
        path = "contracts/UniswapV3Pool.sol"
        for i in (0, 1):
            before = f"feeProtocol{i} >= 4"
            assert baseline[path].count(before) == 1
            baseline[path] = baseline[path].replace(before, f"feeProtocol{i} >= 2")
        if role == "factory":
            path = "contracts/UniswapV3Factory.sol"
            needle = "        feeAmountTickSpacing[500] = 10;"
            assert baseline[path].count(needle) == 1
            baseline[path] = baseline[path].replace(needle, "        feeAmountTickSpacing[100] = 1;\n        emit FeeAmountEnabled(100, 1);\n"+needle)
        assert code == baseline
        witness[role] = dict(address=address, reference=ref, contracts_checked=len(code),
                             compared_hash=hashlib.sha256(json.dumps(code,sort_keys=True).encode()).hexdigest(),
                             actual=actual, original=original)
    tokens = [c.one(POOL, s+"()", returns=["address"])[0] for s in ("token0", "token1")]
    fee, = c.one(POOL,"fee()", returns=["uint24"])
    spacing, = c.one(POOL,"tickSpacing()", returns=["int24"])
    assert c.one(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[*tokens,fee],["address"])[0] == POOL
    assert pool_address(FACTORY,tokens,fee,bytes.fromhex(witness["pool"]["actual"]["init_hash"][2:])) == POOL
    tiers = {str(t): z for t in [100,500,3000,10000] if (z:=c.one(FACTORY,"feeAmountTickSpacing(uint24)",["uint24"],[t],["int24"])[0])}
    slot = c.one(POOL,"slot0()",returns=["uint160","int24","uint16","uint16","uint16","uint8","bool"])
    word = slot[1]//spacing//256
    ticks, = c.one(LENS,"getPopulatedTicksInWord(address,int16)",["address","int16"],[POOL,word],["(int24,int128,uint128)[]"])
    virtual = bytes.fromhex((ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x"))
    side = tokens.index(WETH)
    swap = record(dict(kind="v3",network="base",hash=c.selector["blockHash"],block=snapshot["block"],
                    contracts={"factory":FACTORY},quoter_runtime_keccak="0x"+keccak(virtual).hex(),
                    pools=[dict(idx=0,pool=POOL,tokens=tokens,quotes=[dict(side=side,input=str(i)) for i in (10**12,10**14,10**16,10**17)])]))
    rows = swap["pools"][0]["cases"]
    if len(rows)!=4 or any("sold" not in r for r in rows):
        raise ValueError(rows)
    result = dict(provenance=witness, tokens=tokens, fee=fee, spacing=spacing, tiers=tiers, lens=LENS,
                  lens_word=word, lens_ticks=ticks, reads=c.calls, execution=swap)
    output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8",newline="\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    main(parser.parse_args().output)
