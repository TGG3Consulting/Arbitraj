"""Identify V3 forks by their compiled source, factory and pool on a canonical block."""
import argparse
import hashlib
import json
import posixpath
import re
from pathlib import Path

import requests
from eth_utils import keccak

from record_v2_chain_pools import NETWORKS, view
from record_base_slipstream_executor import ROOT, calldata, rpc


def sources(address,chain):
    url=f"https://sourcify.dev/server/v2/contract/{chain}/{address}"
    res=requests.get(url,params={"fields":"compilation,sources,runtimeBytecode.onchainBytecode"},timeout=18)
    res.raise_for_status()
    data=res.json()
    if not data.get("runtimeMatch"):
        raise ValueError("missing verified runtime")
    entry=data["compilation"]["fullyQualifiedName"].split(":")[0]
    todo=[entry]
    visited={}
    while todo:
        name=todo.pop()
        if name in visited:
            continue
        content=data["sources"][name]["content"]
        code=re.sub(r"//[^\n]*|/\*.*?\*/","",content,flags=re.S)
        imports=re.findall(r"import\s+(?:[^;]*?\sfrom\s+)?[\x22\x27]([^\x22\x27]+)[\x22\x27]\s*;",code)
        visited[name]=hashlib.sha256(content.encode()).hexdigest()
        todo.extend(posixpath.normpath(posixpath.join(posixpath.dirname(name),p)) if p.startswith(".") else p for p in imports)
    return {"source":url,"source_digest":hashlib.sha256(json.dumps(visited,sort_keys=True).encode()).hexdigest(),
            "files":visited,"contract":entry,"runtime":data["runtimeBytecode"]["onchainBytecode"]}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources",type=Path)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    out={}
    chains=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())
    for network in NETWORKS:
        chain=chains[network]
        tag={"blockHash":chain["hash"],"requireCanonical":True}
        url=NETWORKS[network][0]
        data=json.loads((args.sources/f"{network}-integration-2026-11-26/sources/factory-probes.json").read_bytes())["rows"]
        rows=out[network]={}
        for dex,r in data.items():
            if r.get("factory_roundtrip")!="getPool(address,address,uint24)":
                continue
            rec=rows[dex]={"pool":r["pool"],"factory":r["reported_factory"],"network":network,"block":chain["block"],"hash":chain["hash"]}
            try:
                src=sources(r["pool"],chain["chain"])
                code=rpc(url,"eth_getCode",[r["pool"],tag])
                if code.lower()!=src.pop("runtime").lower():
                    raise ValueError("canonical runtime differs from verified deployment")
                fee=int.from_bytes(view(url,tag,r["pool"],calldata("fee()")),"big")
                tokens=[r["token0"],r["token1"]]
                got=view(url,tag,r["reported_factory"],calldata("getPool(address,address,uint24)",["address","address","uint24"],[*tokens,fee]))
                if got[-20:].hex()!=r["pool"][2:].lower():
                    raise ValueError("factory reverse lookup mismatch")
                rec.update(src,fee=fee,tokens=tokens,code_hash="0x"+keccak(bytes.fromhex(code[2:])).hex())
            except (ValueError,KeyError,requests.RequestException) as e:
                rec["reason"]=str(e)[:160]
        print(network,len(rows),flush=True)
        args.output.write_text(json.dumps(out,indent=2)+"\n",encoding="utf8")


if __name__=="__main__":
    main()
