"""Enumerate original Bancor v2.1 liquidity and replay funded wallet payments at one pinned Ethereum state."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT, OWNER, call, calldata, rpc, simulate_block
from record_carbon_ethereum import MULTI, NATIVE, WETH, FUNDING, tx

SOURCE = "https://github.com/bancorprotocol/contracts-solidity/tree/78c5736da250f396616e31e96087cb28e2a5dbbe"
REGISTRY = "0xc0205e203f423bcd8b2a4d6f8c8a154b0aa60f19"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
ZERO = "0x"+"00"*20
UINT = "uint256"


def directory(c):
    registry, = c.one(REGISTRY,"registry()",returns=["address"])
    names = ["BancorNetwork","BancorConverterRegistry","BNTToken","ConversionPathFinder"]
    values = posts(c,[(registry,"addressOf(bytes32)",["bytes32"],[n.encode().ljust(32,bytes(1))]) for n in names])
    roles = dict(zip(names,[decode(["address"],v)[0] for v in values],strict=True), registry=registry)
    if roles["BancorConverterRegistry"] != REGISTRY or any(a == ZERO for a in roles.values()):
        raise ValueError(("unexpected registry",roles))
    anchors, = c.one(REGISTRY,"getLiquidityPools()",returns=["address[]"])
    converters, = c.one(REGISTRY,"getConvertersBySmartTokens(address[])",["address[]"],[anchors],["address[]"])
    if len(set(anchors)) != len(anchors) or len(set(converters)) != len(anchors):
        raise ValueError("ambiguous converter population")
    owners = posts(c,[(a,"owner()",[],[]) for a in anchors])
    counts = posts(c,[(v,"connectorTokenCount()",[],[]) for v in converters])
    query = []
    markets = []
    for a,v,owner,n in zip(anchors,converters,owners,counts,strict=True):
        if decode(["address"],owner)[0] != v:
            raise ValueError(("anchor owner differs from converter registry",a,v))
        count = int.from_bytes(n,"big")
        markets.append(dict(anchor=a,converter=v,count=count))
        query.extend([(v,"connectorTokens(uint256)",[UINT],[i]) for i in range(count)])
    tokens = iter([decode(["address"],item)[0] for item in posts(c,query)])
    for row in markets:
        row["reserves"] = [next(tokens) for _ in range(row["count"])]
        if len(set(row["reserves"])) != row["count"] or ZERO in row["reserves"]:
            raise ValueError(("invalid reserve list",row))
    return roles,markets


def trial(c,router,path,amount,deadline):
    source,target = path[0],path[-1]
    rate = tx(router,"rateByPath(address[],uint256)",["address[]",UINT],[path,amount])
    def swap(minimum):
        return tx(router,"convertByPath(address[],uint256,uint256,address,address,uint256)",
                  ["address[]",UINT,UINT,"address","address",UINT],[path,amount,minimum,OWNER,ZERO,0],amount if source==NATIVE else 0)
    def balance(asset):
        if asset == NATIVE:
            return tx(MULTI,"getEthBalance(address)",["address"],[OWNER])
        return tx(asset,"balanceOf(address)",["address"],[OWNER])
    prep=[]
    if source != NATIVE:
        prep = [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",[UINT,"address[]","address",UINT],
                   [amount,[WETH,source],OWNER,deadline],10**18),
                tx(source,"approve(address,uint256)",["address",UINT],[router,amount])]
    def simulate(calls):
        return simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(20*10**18)}},calls)
    sample=simulate([*prep,rate])
    bad=next(((i,r) for i,r in enumerate(sample) if r["status"]!="0x1"),None)
    if bad:
        return dict(path=path,requested=str(amount),stopped="separate funding or approval" if bad[0]<len(prep) else "rateByPath",error=bad[1].get("error"))
    quoted=int(sample[-1]["returnData"],16)
    if not quoted:return dict(path=path,requested=str(amount),stopped="zero original quote")
    balances=[balance(source),balance(target)]
    result=simulate([*prep,rate,*balances,swap(quoted+1),*balances,swap(quoted),*balances])
    n=len(prep)
    if any(x["status"]!="0x1" for x in result[:n+3]) or int(result[n]["returnData"],16)!=quoted:
        raise ValueError("different funded state")
    rejected,fill = result[n+3],result[n+6]
    if rejected["status"]!="0x0" or result[n+1]["returnData"]!=result[n+4]["returnData"] or result[n+2]["returnData"]!=result[n+5]["returnData"]:
        raise ValueError("original minimum failed to protect balances")
    if fill["status"]!="0x1":
        return dict(path=path,requested=str(amount),quoted=str(quoted),stopped="convertByPath",error=fill.get("error"))
    spent=int(result[n+4]["returnData"],16)-int(result[n+7]["returnData"],16)
    paid=int(result[n+8]["returnData"],16)-int(result[n+5]["returnData"],16)
    if spent!=amount or paid<=0 or int(fill["returnData"],16)!=quoted:
        raise ValueError(("wallet/return mismatch",source,target,spent,paid,quoted))
    return dict(path=path,requested=str(amount),quoted=str(quoted),spent=str(spent),received=str(paid),
                approval_gas=int(result[1]["gasUsed"],16) if prep else 0,gas=int(fill["gasUsed"],16),tx=swap(quoted),
                logs=fill["logs"],strict=rejected.get("error"))


def capture(endpoint):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c=Capture(dict(snap,endpoint=endpoint)); tag=c.selector
    header=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if rpc(endpoint,"eth_chainId",[])!="0x1" or int(header["number"],16)!=snap["block"] or header["hash"]!=tag["blockHash"]:
        raise ValueError("wrong network or block")
    roles,markets=directory(c)
    anchors={r["anchor"]:r for r in markets}
    router,bnt=roles["BancorNetwork"],roles["BNTToken"]
    controls=[]
    sizes={USDT:[10**6,8*10**6],USDC:[10**6,8*10**6],NATIVE:[10**14,10**15],bnt:[10**17,10**18]}
    pairs=[(NATIVE,bnt),(USDT,bnt),(USDC,bnt),(NATIVE,USDT),(USDC,USDT)]
    for a,b in pairs:
        for src,dst in ((a,b),(b,a)):
            path,=c.one(router,"conversionPath(address,address)",["address","address"],[src,dst],["address[]"])
            if len(path)<3 or len(path)%2!=1 or path[0]!=src or path[-1]!=dst:
                raise ValueError(("invalid native conversion path",src,dst,path))
            for j in range(1,len(path),2):
                if not {path[j-1],path[j+1]}.issubset(anchors[path[j]]["reserves"]):
                    raise ValueError(("path not registered by reserves",path[j]))
            controls.extend(trial(c,router,path,qty,int(header["timestamp"],16)+600) for qty in sizes[src])
    if not any(row.get("received") for row in controls if len(row["path"])==3) or not any(row.get("received") for row in controls if len(row["path"])>3):
        raise ValueError(("no funded single or composed routes",[(r["path"][0],r.get("stopped")) for r in controls]))
    roles_proof={}
    for name,address in roles.items():
        bytecode=bytes.fromhex(rpc(endpoint,"eth_getCode",[address,tag])[2:])
        if not bytecode:raise ValueError(("empty registry role",name))
        roles_proof[name]=dict(address=address,keccak256="0x"+keccak(bytecode).hex(),bytes=len(bytecode))
    return dict(network="ethereum",chain_id=1,block=snap["block"],selector=tag,state_root=header["stateRoot"],
                source=SOURCE,roles=roles_proof,markets=markets,controls=controls)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path)
    args=parser.parse_args();errors=[]
    for endpoint in ("https://rpc.mevblocker.io","https://eth.drpc.org"):
        try:
            result=capture(endpoint)
            break
        except (ValueError,RuntimeError,requests.RequestException,KeyError) as error:
            errors.append((endpoint,str(error)[:240]))
    else:
        raise ValueError(errors)
    args.output.write_bytes(json.dumps(result,separators=(",",":")).encode())
