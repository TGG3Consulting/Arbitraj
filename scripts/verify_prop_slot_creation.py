"""Rebuild both Ethereum slot-v2 wallets by read-only constructor calls on the recorded block."""
import gzip
import json
import sys
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_ethereum_x7 import ENDPOINTS
from record_prop_venue import rpc
from record_safeswap_protected import OWNER, runtime_with_args
from verify_wallet_creation import executable, sha


def verify(compiled, previous, endpoint):
    at = {"blockHash": previous["sourceBlock"], "requireCanonical": True}
    client = Capture({"selector": at, "endpoint": endpoint})
    assert int(rpc(client, "eth_chainId", [])["result"], 16) == previous["chainId"] == 1
    creation = bytes.fromhex(compiled["creation"])
    prior_init = bytes.fromhex(previous["creation"][2:])
    assert executable(prior_init) == executable(creation)
    markets, metrics, runtimes = {}, {}, {}
    types = ["address"]*3 + ["bytes32"]*3
    for venue, old in previous["markets"].items():
        addresses = old["addresses"]
        assert addresses[1] == venue
        shells = [keccak(hexstr=rpc(client, "eth_getCode", [a, at])["result"]) for a in addresses]
        assert ["0x"+h.hex() for h in shells] == old["shells"]
        roles = addresses + shells
        args = encode(types, roles)
        assert old["creationSha256"] == sha(prior_init+args)
        data = {"from": OWNER, "data": "0x"+(prior_init+args).hex()}
        prior = bytes.fromhex(rpc(client, "eth_call", [data, at])["result"][2:])
        assert old["runtimeSha256"] == sha(prior) and old["runtimeKeccak"] == "0x"+keccak(prior).hex()
        runtime = bytes.fromhex(runtime_with_args(client, compiled, types, roles)[2:])
        assert executable(runtime) == executable(prior)
        row = dict(old, creationSha256=sha(creation+args), runtimeSha256=sha(runtime),
                   runtimeKeccak="0x"+keccak(runtime).hex())
        markets[venue] = row
        runtimes[venue] = {"previousRuntime":"0x"+prior.hex(), "currentRuntime":"0x"+runtime.hex()}
        metrics[venue] = dict(previousRuntime=old["runtimeKeccak"],runtime=row["runtimeKeccak"],
                              identicalRuntimeBytes=len(executable(runtime)),
                              runtimeProgramSha256=sha(executable(runtime)))
    return (dict(previous,creation="0x"+compiled["creation"],markets=markets),
            dict(sourceBlock=at["blockHash"], identicalInitBytes=len(executable(creation)), markets=metrics), runtimes)


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    compiled = json.loads(Path(sys.argv[1]).read_bytes())
    previous = json.loads((root/"ui/src/assets/prop-slot-v2-ethereum-previous.json").read_bytes())
    for endpoint in ENDPOINTS:
        try:
            card, metrics, runtimes = verify(compiled, previous, endpoint)
            break
        except (OSError, KeyError, RuntimeError, ValueError) as err:
            last = err
    else:
        raise RuntimeError("No Ethereum RPC returned the recorded constructors") from last
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    evidence = {"verification": metrics, "markets":runtimes}
    (out/"prop_slot_creation_v2.json.gz").write_bytes(gzip.compress(json.dumps(evidence,separators=(",",":")).encode(),mtime=0))
    for name, obj in [("prop-slot-v2-ethereum.json",card),("verified.json",metrics)]:
        (out/name).write_text(json.dumps(obj,separators=(",",":"))+"\n")
