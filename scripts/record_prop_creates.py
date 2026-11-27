"""Reproduce each PropVenueAdapter constructor using canonical Ethereum eth_call with no state override."""
import argparse
import hashlib
import json
from pathlib import Path
from eth_abi import encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_ethereum_x7 import ENDPOINTS
from record_prop_cash_sources import CASES, REFERENCE
from record_prop_venue import ROUTER, rpc
from record_safeswap_protected import runtime_with_args


def venues():
    profiles=json.loads(Path("config/first_run_dex.json").read_text(encoding="utf8"))
    found={}
    for row in profiles:
        settings=row["settings"]
        if row["chain_id"]!=1 or settings.get("dex.kind")!="lambda_propamm":
            continue
        roles={r[0]:r[1:] for r in (line.split() for line in settings["dex.propamm_roles"])}
        venue=roles["venue"][0].lower()
        if roles["router"][0].lower()!=ROUTER or venue!=settings["dex.factory"].lower():
            raise ValueError("profile/cash mismatch")
        found[venue]=(roles["vault"][0].lower(),settings["dex.module_runtime_hashes"])
    for venue,vault,_ in CASES.values():
        if found.setdefault(venue,(vault,[]))[0]!=vault:
            raise ValueError("historical payer differs from configured vault")
    return found


def capture(endpoint, compiled, bound=False):
    height, block = next(iter(REFERENCE.items()))
    selector={"blockHash":block,"requireCanonical":True}
    client=Capture({"endpoint":endpoint,"selector":selector})
    header=rpc(client,"eth_getBlockByNumber",[hex(height),False])["result"]
    if header["hash"]!=block or int(rpc(client,"eth_chainId",[])["result"],16)!=1:
        raise ValueError("foreign chain or block")
    out={}
    types=["address"]*3+["bytes32"]*3
    for venue,(vault,accepted) in venues().items():
        if bound and venue not in {case[0] for case in CASES.values()}:continue
        addresses=[ROUTER,venue,vault]
        code=[bytes.fromhex(rpc(client,"eth_getCode",[a,selector])["result"][2:]) for a in addresses]
        if not all(code):
            raise ValueError("missing role code")
        hashes=[keccak(c) for c in code]
        values=addresses+hashes
        runtime=bytes.fromhex(runtime_with_args(client,compiled,types,values)[2:])
        runtime_hash="0x"+keccak(runtime).hex()
        if bound:
            import gzip
            name=next(key for key,case in CASES.items() if case[0]==venue)
            archive=Path("tests/data/first_run_dex_2026-11-26")/(name+"_ethereum_bound_wallet.json.gz")
            expected={r["runtime"] for r in json.loads(gzip.decompress(archive.read_bytes()))["builders"]}
            if expected!={runtime_hash}:raise ValueError("slot wallet differs from the funded replay")
        if accepted and runtime_hash not in accepted:
            raise ValueError("constructor differs from accepted wallet runtime for "+venue)
        data=bytes.fromhex(compiled["creation"])+encode(types,values)
        out[venue]={"addresses":addresses,"shells":["0x"+h.hex() for h in hashes],
                    "creationSha256":hashlib.sha256(data).hexdigest(),
                    "runtimeSha256":hashlib.sha256(runtime).hexdigest(),"runtimeKeccak":runtime_hash}
    return {"kind":"lambda_propamm","chainId":1,"sourceBlock":block,"creation":"0x"+compiled["creation"],"markets":out}


if __name__=="__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("compiled",type=Path)
    parser.add_argument("output",type=Path)
    parser.add_argument("--bound",action="store_true")
    args=parser.parse_args()
    artifact=json.loads(args.compiled.read_text())
    for endpoint in ENDPOINTS:
        try:
            result=capture(endpoint,artifact,args.bound)
        except Exception as e:
            print(type(e).__name__,str(e)[:180],flush=True)
            continue
        args.output.write_text(json.dumps(result,separators=(",",":"))+"\n")
        break
    else:
        raise RuntimeError("no archive completed all constructors")

