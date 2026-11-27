"""Verify a V3 deployment profile against a known implementation and sample its supported fee tiers."""
import argparse
import json
from pathlib import Path
from eth_utils import keccak
from record_base_slipstream_executor import ROOT, calldata, rpc
from record_v2_chain_pools import NETWORKS, view
from record_verified_v3_sources import sources
from record_v3_fee_registry import candidate_fees


def verify(network, rows, raw):
    profiles=json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    url=NETWORKS[network][0]
    chain=8453 if network=="base" else 1
    known_fees = candidate_fees(profiles, ROOT/"tests/data/first_run_dex_2026-11-26")
    found={}
    for dex,row in rows.items():
        ref=row["reference"]
        ref_profile=next(p for p in profiles if p["network"]==network and p["id"]==ref)
        if not all(c.get("sold") for c in row.get("execution",{}).get("pools",[{"cases":[]}])[0]["cases"]):
            continue
        tag={"blockHash":row["hash"],"requireCanonical":True}
        def call(a,s,t=(),args=()):
            return view(url,tag,a,calldata(s,t,args))
        def proof(a):
            s=sources(a,chain)
            runtime=s.pop("runtime")
            code=rpc(url,"eth_getCode",[a,tag])
            if runtime.lower()!=code.lower():
                raise ValueError("verified runtime differs from pinned block")
            return {"address":a,"contract":s["contract"],"source":s["source"],"digest":s["source_digest"],"keccak":"0x"+keccak(bytes.fromhex(code[2:])).hex(),"bytes":(len(code)-2)//2}
        r=found[dex]={}
        try:
            factory=row["factory"]
            reference_factory=raw[ref]["factory"]
            r["factory"]=proof(factory);r["reference_factory"]=proof(reference_factory)
            if r["factory"]["digest"]!=r["reference_factory"]["digest"]:
                raise ValueError("factory source differs")
            if "pancake" in ref:
                ref_deployer="0x"+call(reference_factory,"poolDeployer()")[-20:].hex()
                deployer="0x"+call(factory,"poolDeployer()")[-20:].hex()
                r["reference_deployer"]=proof(ref_deployer)
                code=rpc(url,"eth_getCode",[deployer,tag])
                digest="0x"+keccak(bytes.fromhex(code[2:])).hex()
                if digest==r["reference_deployer"]["keccak"]:
                    r["deployer"]={**r["reference_deployer"],"address":deployer,"runtime_equals":ref_deployer}
                else:
                    r["deployer"]=proof(deployer)
                if r["deployer"]["digest"]!=r["reference_deployer"]["digest"]:
                    raise ValueError("deployer source differs")
            lens=ref_profile["settings"]["dex.tick_lens"]
            r["lens"]=lens
            state=call(row["pool"],"slot0()")
            tick=int.from_bytes(state[32:64],"big",signed=True)
            spacing=int.from_bytes(call(row["pool"],"tickSpacing()"),"big",signed=True)
            word=(tick//spacing)>>8
            ticks=call(lens,"getPopulatedTicksInWord(address,int16)",["address","int16"],[row["pool"],word])
            r["lens_word"]=word;r["lens_result"]="0x"+ticks.hex()
            r["fee_tiers"]={}
            # An independently witnessed registration is a candidate, not proof of availability here.
            for fee in sorted(known_fees | {row["fee"],1,5,10,25,50,100,200,250,300,400,500,1000,2000,2500,3000,5000,7500,10000,20000}):
                delta=int.from_bytes(call(factory,"feeAmountTickSpacing(uint24)",["uint24"],[fee]),"big",signed=True)
                if delta>0:r["fee_tiers"][str(fee)]=delta
            if r["fee_tiers"].get(str(row["fee"]))!=spacing:
                raise ValueError("sample fee or tick spacing mismatch")
            r["admitted"]=True
        except Exception as e:r["reason"]=str(e)[:180]
        print(network,dex,r.get("admitted",r.get("reason")),flush=True)
    return found


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("source",type=Path);p.add_argument("executor",type=Path);p.add_argument("output",type=Path)
    args=p.parse_args()
    raw=json.loads(args.source.read_bytes());markets=json.loads(args.executor.read_bytes())
    result={n:verify(n,r,raw[n]) for n,r in markets.items()}
    args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
