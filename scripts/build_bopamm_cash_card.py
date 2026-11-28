"""Assemble the Ethereum wallet card from the GitHub-compiled artifact and independent funded simulations."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

from eth_utils import keccak

ROOT=Path(__file__).resolve().parents[1]
DATA=ROOT / "tests/data/first_run_dex_2026-11-26"
UI=ROOT / "ui/src/assets"

def pack(compiled, frames, comparison, first, run):
    card=json.loads((UI / "bopamm-ethereum-create.json").read_bytes())
    codes={sample.pop("runtime") for f in frames for row in f["cases"] for sample in (row["compound"],row["direct"]) if sample["outcome"]=="paid"}
    assert len(codes)==1
    runtime=codes.pop()
    deployed=bytes.fromhex(runtime[2:])
    template=bytearray.fromhex(compiled["runtime"])
    args=[card["roles"]["market"]["address"],card["roles"]["pricing"]["address"],card["usdc"]]
    groups={}
    for group,refs in compiled["immutableReferences"].items():
        words={deployed[r["start"]:r["start"]+32] for r in refs if r["length"]==32}
        assert len(words)==1 and all(r["length"]==32 for r in refs)
        word=words.pop()
        groups[group]="0x"+word[-20:].hex()
        assert word==bytes.fromhex(groups[group][2:]).rjust(32,b"\0") and groups[group] in args
        for r in refs:
            template[r["start"]:r["start"]+32]=word
    assert bytes(template)==deployed
    for f,old,prior in zip(frames,comparison,first,strict=True):
        assert (f["builder"],f["parent"])==(old["builder"],old["parent"])==(prior["builder"],prior["parent"])
        for row,other,initial in zip(f["cases"],old["cases"],prior["cases"],strict=True):
            assert (row["input"],row["amount"])==(other["input"],other["amount"])==(initial["input"],initial["amount"])
            row["lambda"]=other["direct"]
            row["nested_module"]={k:v for k,v in initial["compound"].items() if k in ("gas","spent","received","outcome")}
    create=bytes.fromhex(compiled["creation"])+b"".join(bytes.fromhex(a[2:]).rjust(32,b"\0") for a in args)
    card.update(creation="0x"+compiled["creation"],moduleRole="cash_module",artifactRun=run,
        creationSha256=hashlib.sha256(create).hexdigest(),runtimeSha256=hashlib.sha256(deployed).hexdigest(),runtimeKeccak="0x"+keccak(deployed).hex())
    proof=dict(artifactRun=run,lambdaRun=46103071108,nestedRun=46104392774,runtime=runtime,immutableGroups=groups,frames=frames)
    return card,proof

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ("compiled","frames","lambda_frames","first_module"):parser.add_argument(name,type=Path)
    parser.add_argument("run",type=int)
    args=parser.parse_args()
    card,proof=pack(*(json.loads(p.read_bytes()) for p in [args.compiled,args.frames,args.lambda_frames,args.first_module]),args.run)
    (UI/"bopamm-cash-ethereum-create.json").write_text(json.dumps(card,separators=(",",":"))+"\n",encoding="utf8")
    (DATA/"bopamm_ethereum_cash_wallet.json.gz").write_bytes(gzip.compress(json.dumps(proof,separators=(",",":")).encode(),mtime=0))
