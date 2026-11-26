"""Read Metric V1 registrations, pricing and exact-input swap quotes at the recorded chain block."""
import argparse
import hashlib
import io
import json
import re
import tarfile
from pathlib import Path

import requests
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata, rpc
from record_v2_chain_pools import NETWORKS

SDK = "https://registry.npmjs.org/@nradko/metric-omm-sdk-v1/-/metric-omm-sdk-v1-0.4.5.tgz"
VIRTUAL = "0x" + keccak(text="arb.virtual_quoter")[-20:].hex()


def read_call(url, tag, target, data, override=None):
    params = [{"to": target, "data": data}, tag]
    if override:
        params.append(override)
    response = requests.post(url, json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":params}, timeout=18)
    response.raise_for_status()
    body = response.json()
    if "error" in body:
        raise ValueError(str(body["error"].get("message"))[:90])
    return bytes.fromhex(body["result"][2:])


def address(raw):
    if len(raw) != 32:
        raise ValueError("bad address length")
    return "0x" + raw[-20:].hex()


def capture(network, contracts, artifact_hash):
    chains = json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())
    chain = chains[network]
    url, wrapped = NETWORKS[network]
    tag = {"blockHash":chain["hash"],"requireCanonical":True}
    if int(rpc(url, "eth_chainId", []),16) != chain["chain"]:
        raise ValueError("wrong chain")
    factory = contracts["factory"]
    def view(target, sig, types=(), args=()):
        return read_call(url,tag,target,calldata(sig,types,args))
    runtime = bytes.fromhex((ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x"))
    override = {VIRTUAL:{"code":"0x"+runtime.hex()}}
    n = int.from_bytes(view(factory,"nextPoolIdx()"),"big")
    recorded = {"network":network,"block":chain["block"],"hash":chain["hash"],"sdk":SDK,
                "sdk_sha256":artifact_hash,"contracts":contracts,"next_pool_idx":n,
                "quoter_runtime_keccak":"0x"+keccak(runtime).hex(),"pools":[]}
    # Reporting both successful and rejected samples preserves the oracle and pool boundaries.
    for index in range(1,n):
        pool = address(view(factory,"idxToPool(uint256)",["uint256"],[index]))
        row = {"idx":index,"pool":pool,"quotes":[]}
        recorded["pools"].append(row)
        if int.from_bytes(view(factory,"isPool(address)",["address"],[pool]),"big")!=1:
            row["reason"]="factory.isPool false"
            continue
        try:
            imm=view(pool,"getImmutables()")
            if len(imm)!=24*32 or address(imm[:32]).lower()!=factory.lower():
                raise ValueError("getImmutables ABI or factory mismatch: "+str(len(imm)))
            tokens=[address(imm[i*32:(i+1)*32]) for i in (1,2)]
            row.update(tokens=tokens,immutables="0x"+imm.hex())
            price=view(pool,"getSellAndBuyPrices()")
            row["prices"]="0x"+price.hex()
            decimals=[int.from_bytes(view(a,"decimals()"),"big") for a in tokens]
            stable = {"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913","0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","0xdac17f958d2ee523a2206206994597c13d831ec7"}
            quote_side = next((i for i,t in enumerate(tokens) if t in stable),None)
            if quote_side is None:
                quote_side = tokens.index(wrapped) if wrapped in tokens else None
            if quote_side is None:
                raise ValueError("no configured funding asset in this sample")
            divisors = (1,10) if tokens[quote_side] in stable else (1000,10000)
            for divisor in divisors:
                amount = 10**decimals[quote_side]//divisor
                for side in (quote_side,1-quote_side):
                    q=read_call(url,tag,VIRTUAL,calldata("quoteMetric(address,bool,uint256)",["address","bool","uint256"],[pool,side==0,amount]),override)
                    if len(q)!=64:
                        raise ValueError("bad virtual quote ABI")
                    used,out=[int.from_bytes(q[j:j+32],"big") for j in (0,32)]
                    row["quotes"].append({"side":side,"input":str(amount),"used":str(used),"out":str(out)})
                    if used!=amount or not out:
                        break
                    amount=out
        except (ValueError,requests.RequestException) as error:
            row["reason"]=str(error)[:100]
        ok=[p for p in recorded["pools"] if len(p["quotes"])==4 and all(q["input"]==q["used"] and int(q["out"]) for q in p["quotes"])]
        if len(ok)>=3:
            break
    return recorded


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("network",choices=NETWORKS)
    parser.add_argument("output",type=Path)
    parser.add_argument("--generation",choices=["current","retired"],default="current")
    args=parser.parse_args()
    archive=requests.get(SDK,timeout=15)
    archive.raise_for_status()
    with tarfile.open(fileobj=io.BytesIO(archive.content)) as t:
        ts=t.extractfile("package/src/addresses.ts").read().decode()
    contracts=dict(re.findall(r'^  (factory|quoter): "(0x\w+)"',ts,re.M))
    if args.generation=="current":
        # Same factory as the bot's recorded BSC deployment; its registry, pool immutables
        # and canonical runtime are queried independently on the requested chain.
        known=json.loads((ROOT/"tests/data/metric_2026-11-15/probe.json").read_bytes())
        contracts={"factory":known["factory"]}
    result=capture(args.network,contracts,hashlib.sha256(archive.content).hexdigest())
    result["generation"]=args.generation
    args.output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+"\n",encoding="utf8")
    print(args.network,len(result["pools"]),sum(len(r["quotes"]) for r in result["pools"]))
