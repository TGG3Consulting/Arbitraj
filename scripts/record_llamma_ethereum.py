"""Ethereum LLAMMA directory and physical quotes from three distinct canonical factory registries.

Curve's integration guide and deployed Vyper factories describe these getters.
https://docs.curve.finance/developer/integration/llamma/
https://docs.curve.finance/developer/deployments/lending
"""
import argparse
import collections
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from fork_fee_rpc import Reader
from record_base_slipstream_executor import ROOT
from record_llama_base_cl import CanonicalCalls
from record_mooniswap import aggregate

FACTORIES = {
    "oneway": "0xea6876dde9e3467564acbee1ed5bac88783205e0",
    "v2": "0x8f6b56ec5ddf1f2691a1059f1d3cd97ac9eab0bd",
    "mint": "0xc9332fdcb1c491dcc683bae86fe3cb70360738bc",
}
EVENT = "0x"+keccak(text="TokenExchange(address,uint256,uint256,uint256,uint256)").hex()
TRANSFER = "0x"+keccak(text="Transfer(address,address,uint256)").hex()
FIRST_BLOCK = 26487814  # Canonical UTC trading day through H26494988.
UINT = "uint256"


def markets(cap, kind, factory):
    count, = cap.one(factory, "n_collaterals()" if kind=="mint" else "market_count()", returns=[UINT])
    if kind=="v2":
        raw = aggregate(cap, [(factory,"markets(uint256)",[UINT],[i]) for i in range(count)])
        fields = ("vault","controller","amm","collateral","borrowed","oracle","policy")
        rows = [dict(zip(fields,decode(["address"]*len(fields),r),strict=True)) for r in raw]
    else:
        getters = [("amm","amms"),("controller","controllers")]
        getters += [("collateral","collaterals")] if kind=="mint" else [
            ("collateral","collateral_tokens"),("borrowed","borrowed_tokens"),("vault","vaults")]
        raw = iter(aggregate(cap,[(factory,f"{g}(uint256)",[UINT],[i]) for i in range(count) for _,g in getters]))
        rows = [{key:decode(["address"],next(raw))[0] for key,_ in getters} for _ in range(count)]
    coins = iter(aggregate(cap,[(m["amm"],"coins(uint256)",[UINT],[i]) for m in rows for i in (0,1)]))
    for i, m in enumerate(rows):
        borrowed,collateral = [decode(["address"],next(coins))[0] for _ in (0,1)]
        if borrowed==collateral or m["collateral"]!=collateral or m.get("borrowed",borrowed)!=borrowed:
            raise ValueError(("foreign AMM currencies",kind,i))
        m.update(index=i,borrowed=borrowed)
    if len({m["amm"] for m in rows})!=count:
        raise ValueError(("duplicate market",kind))
    return rows


def quotes(cap, rows):
    assets = list(dict.fromkeys(a for m in rows for a in (m["borrowed"],m["collateral"])))
    decimals = {a:decode([UINT],raw)[0] for a,raw in zip(assets,aggregate(cap,[(a,"decimals()",[],[]) for a in assets]),strict=True) if raw}
    requests,keys = [],[]
    for m in rows:
        for i, asset in enumerate([m["borrowed"],m["collateral"]]):
            if asset not in decimals or decimals[asset]>36:
                continue
            for n in dict.fromkeys([max(1,10**decimals[asset]//1000),10**decimals[asset]]):
                requests.append((m["amm"],"get_dxdy(uint256,uint256,uint256)",[UINT]*3,[i,1-i,n]))
                keys.append((m["index"],i,n))
    result = []
    for (idx,i,n),raw in zip(keys,aggregate(cap,requests),strict=True):
        q = decode([UINT,UINT],raw) if raw and len(raw)==64 else None
        if q and q[0]>n:
            raise ValueError(("quote over cap",idx,i))
        result.append(dict(index=idx,side=i,cap=str(n),spent=str(q[0]) if q else None,received=str(q[1]) if q else None))
    return result


def evidence(reader, rows_by_kind, last):
    all_markets = {m["amm"]:m for rows in rows_by_kind.values() for m in rows}
    if len(all_markets)!=sum(map(len,rows_by_kind.values())):
        raise ValueError("shared AMM across canonical registries")
    end = reader.one("eth_getBlockByHash",[reader.tag["blockHash"],False])
    if int(end["number"],16)!=last:
        raise ValueError("height mismatch")
    addresses=list(all_markets)
    counts=collections.Counter()
    example={}
    for i in range(0,len(addresses),16):
        for start in range(FIRST_BLOCK,last+1,1600):
            logs=reader.one("eth_getLogs",[{"address":addresses[i:i+16],"topics":[EVENT],"fromBlock":hex(start),"toBlock":hex(min(last,start+1599))}])
            for log in logs:
                amm=log["address"].lower()
                if amm not in all_markets or log.get("removed") or len(log["topics"])!=2:
                    raise ValueError("foreign swap log")
                counts[amm]+=1
                example.setdefault(amm,log)
    samples=[]
    for amm,log in example.items():
        m=all_markets[amm];tokens=[m["borrowed"],m["collateral"]]
        i,spent,j,out=decode([UINT]*4,bytes.fromhex(log["data"][2:]))
        if (i,j) not in ((0,1),(1,0)):raise ValueError("invalid currency indices")
        recipient="0x"+log["topics"][1][-40:].lower()
        receipt=reader.one("eth_getTransactionReceipt",[log["transactionHash"]])
        if receipt["status"]!="0x1" or receipt["blockHash"]!=log["blockHash"]:
            raise ValueError("detached public settlement")
        transfers=[(v["address"].lower(),"0x"+v["topics"][1][-40:].lower(),"0x"+v["topics"][2][-40:].lower(),int(v["data"],16))
                   for v in receipt["logs"] if len(v["topics"])==3 and v["topics"][0]==TRANSFER and len(v["data"])==66]
        paid=[src for token,src,dst,n in transfers if (token,dst,n)==(tokens[i],amm,spent)]
        credited=sum((token,src,dst,n)==(tokens[j],amm,recipient,out) for token,src,dst,n in transfers)
        samples.append(dict(amm=amm,hash=receipt["transactionHash"],blockHash=receipt["blockHash"],recipient=recipient,
                            signer=receipt["from"],input=tokens[i],output=tokens[j],spent=str(spent),received=str(out),
                            matchingPayers=paid,matchingCredits=credited))
    return dict(first=FIRST_BLOCK,last=last,swaps=counts,samples=samples)


def capture():
    head=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    reader=Reader("ethereum",head)
    cap=CanonicalCalls(reader)
    rows={kind:markets(cap,kind,factory) for kind,factory in FACTORIES.items()}
    last=int(reader.header["number"],16)
    result=dict(selector=reader.tag,root=reader.header["stateRoot"],factories=FACTORIES,markets=rows,
                quotes={k:quotes(cap,v) for k,v in rows.items()},activity=evidence(reader,rows,last))
    if cap.reader.one("eth_getBlockByHash",[reader.tag["blockHash"],False])["stateRoot"]!=result["root"]:
        raise ValueError("different root")
    return result


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);args=ap.parse_args()
    args.output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf-8")

