"""Replay verified-source Uniswap or Pancake V3 forks with the production virtual quoter and V5."""
import argparse
import json
from pathlib import Path

from eth_utils import keccak

from record_pool_executor_controls import record
from record_base_slipstream_executor import ROOT
from record_v2_chain_pools import NETWORKS

STABLES={"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913","0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","0xdac17f958d2ee523a2206206994597c13d831ec7"}
REFS={"base":["uniswap-v3-base","pancakeswap-v3-base"],"ethereum":["uniswap_v3","pancakeswap-v3-ethereum"]}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources",type=Path)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    src=json.loads(args.sources.read_bytes())
    code=bytes.fromhex((ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x"))
    out={}
    for network,items in src.items():
        result=out[network]={}
        weth=NETWORKS[network][1]
        for dex,r in items.items():
            ref=next((ident for ident in REFS[network] if r.get("source_digest") and r["source_digest"]==items[ident].get("source_digest")),None)
            if not ref or dex in REFS[network]:
                continue
            row=result[dex]={k:r[k] for k in ["hash","source","source_digest","code_hash","pool","factory","fee","tokens","block"]}
            row["reference_pool"]=items[ref]["pool"]
            row["reference"]=ref
            side=next((i for i,a in enumerate(r["tokens"]) if a==weth),None)
            if side is None:
                side=next((i for i,a in enumerate(r["tokens"]) if a in STABLES),None)
            if side is None:
                row["reason"]="no sampled WETH or supported stable";continue
            amounts=[10**12,10**14] if r["tokens"][side]==weth else [10**4,10**6]
            target={"idx":0,"pool":r["pool"],"tokens":r["tokens"],"quotes":[{"input":str(a),"side":side} for a in amounts]}
            requested={"kind":"v3","network":network,"hash":r["hash"],"block":r["block"],"contracts":{"factory":r["factory"]},"quoter_runtime_keccak":"0x"+keccak(code).hex(),"pools":[target]}
            row["execution"]=record(requested)
            print(network,dex,[("sold" in x) for x in row["execution"]["pools"][0]["cases"]],flush=True)
            args.output.write_text(json.dumps(out,indent=2)+"\n",encoding="utf8")
    args.output.write_text(json.dumps(out,indent=2)+"\n",encoding="utf8")


if __name__=="__main__":
    main()
