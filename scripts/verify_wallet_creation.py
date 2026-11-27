"""Build the EVM wallet card from a simulated constructor on the recorded canonical block.
Only the isolated owner receives test gas. Both constructors run independently at the same nonce.
The application bytecode, immutable references, constructor arguments and the previous card are checked.
"""
import hashlib
import json

from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, OWNER


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def executable(raw):
    size = int.from_bytes(raw[-2:], "big")
    assert 0 < size < len(raw) - 2
    return raw[:-size - 2]


def verify(compiled, out, previous_card, card_name, chain_id, target_role, rpc):
    data = ROOT / "ui/src/assets"
    old = json.loads((data / previous_card).read_bytes())
    assert old["chainId"] == chain_id and compiled["solc"] == old["solc"]
    args = encode(["address", "address"], [old[target_role], old["wrapped"]])
    old_init = bytes.fromhex(old["data"][2:])
    assert sha(old_init) == old["creationSha256"] and old_init.endswith(args)
    creation = bytes.fromhex(compiled["creation"])
    assert executable(old_init[:-len(args)]) == executable(creation)
    at = {"blockHash": old["sourceBlock"], "requireCanonical": True}
    assert int(rpc("eth_chainId", []), 16) == old["chainId"]
    assert int(rpc("eth_getTransactionCount", [OWNER, at]), 16) == 0
    def constructor(init):
        call = {"from": OWNER, "data": "0x" + init.hex(), "gas": hex(5_000_000)}
        frame = {"stateOverrides": {OWNER: {"balance": hex(10**18)}}, "calls": [call]}
        block, = rpc("eth_simulateV1", [{"blockStateCalls": [frame]}, at])
        assert block["parentHash"] == at["blockHash"]
        row, = block["calls"]
        assert row["status"] == "0x1"
        return bytes.fromhex(row["returnData"][2:]), int(row["gasUsed"], 16)
    previous, old_gas = constructor(old_init)
    assert sha(previous) == old["runtimeSha256"] and "0x"+keccak(previous).hex() == old["runtimeKeccak"]
    assert old_gas == old["gas"]
    init = creation + args
    runtime, gas = constructor(init)
    assert executable(runtime) == executable(previous)
    fresh = dict(old, data="0x"+init.hex(), creationSha256=sha(init), runtimeSha256=sha(runtime),
                 runtimeKeccak="0x"+keccak(runtime).hex(), gas=gas)
    out.mkdir(parents=True, exist_ok=True)
    (out/card_name).write_text(json.dumps(fresh,separators=(",",":"))+"\n")
    metrics = dict(sourceBlock=at["blockHash"], previousGas=old_gas, gas=gas,
                   previousRuntime=old["runtimeKeccak"], runtime=fresh["runtimeKeccak"],
                   identicalRuntimeBytes=len(executable(runtime)), identicalInitBytes=len(executable(creation)),
                   runtimeSha256BeforeMetadata=sha(executable(runtime)))
    (out/"checked.json").write_text(json.dumps(metrics,indent=2)+"\n")
    for name, code in [("previous", previous), ("current", runtime)]:
        (out/(name+"-runtime.hex")).write_text(code.hex()+"\n")
