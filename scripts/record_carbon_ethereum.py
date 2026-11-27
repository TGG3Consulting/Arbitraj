"""Carbon V1 Ethereum. Enumerate every strategy at one canonical block, then settle real
controller calldata with a separately funded wallet. No signatures or submissions."""
import argparse
from dataclasses import dataclass
import json
from pathlib import Path
from eth_abi import decode
from eth_utils import keccak
import requests
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import OWNER, call, calldata, rpc, simulate_block
from record_ethereum_v2_factories import WETH, STABLES, FUNDING, digest
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

CTRL="0xc537e898cd774e2dcba3b14ea6f34c93d5ea45e1"
IMPL="0x1e08049c180e25478bf7a0357171547b7332bf2c"
NATIVE="0x"+"ee"*20
BLOCK="0x2372196a2961b307333be48d3dad74a2e723fd459c2cb01e225a0a97f50e7a7a"
REV="4a1888643eb094310c9ac6ae93f1ba8eaaa9822c"
STRATEGY="(uint256,address,address[2],(uint128,uint128,uint64,uint64)[2])"
ACTION="(uint256,uint128)[]"
MULTI="0xca11bde05977b3631167028862be2a173976ca11"

def tx(address,name,types,values,value=0):
    return call(address,calldata(name,types,values),value)

@dataclass(frozen=True)
class Market:
    controller: str
    wrapped: str
    funding: str


ETH_MARKET=Market(CTRL,WETH,FUNDING)


class InsufficientOrderLiquidity(ValueError):
    """The original trade, after funding, failed its gross target inventory check."""


def collect(c,controller=CTRL):
    pairs,=c.one(controller,"pairs()",[],[],["address[2][]"])
    if len(set(pairs))!=len(pairs):raise ValueError("duplicate Carbon pair")
    operations=[(controller,name,["address","address"],p) for p in pairs for name in ("strategiesByPairCount(address,address)","pairTradingFeePPM(address,address)")]
    meta=[int.from_bytes(r,"big") for r in posts(c,operations)]
    directory=[dict(tokens=p,count=meta[i*2],fee_ppm=meta[i*2+1],orders=[]) for i,p in enumerate(pairs)]
    cursor=[(row,offset,min(offset+32,row["count"])) for row in directory for offset in range(0,row["count"],32)]
    for start in range(0,len(cursor),6):
        pages=cursor[start:start+6]
        ops=[(controller,"strategiesByPair(address,address,uint256,uint256)",["address","address","uint256","uint256"],[*r["tokens"],lo,hi]) for r,lo,hi in pages]
        for (r,lo,hi),raw in zip(pages,posts(c,ops),strict=True):
            rows,=decode([STRATEGY+"[]"],raw)
            if len(rows)!=hi-lo or any(set(row[2])!=set(r["tokens"]) for row in rows):raise ValueError("wrong strategy members")
            r["orders"].extend(rows)
    ids=[s[0] for row in directory for s in row["orders"]]
    if len(ids)!=len(set(ids)) or len(ids)!=sum(row["count"] for row in directory):raise ValueError("incomplete strategy population")
    return directory

def wallet(c,row,source,target,amount,by_target,end,market=None):
    if market is None:market=ETH_MARKET
    # The controller accepts order IDs, not a pair-wide implicit swap. Verify fills against each
    # order's actual remaining target inventory. Compare all eligible single-order allocations.
    eligible=[]
    for sid,_,tokens,orders in row["orders"]:
        y,_,a,b=orders[tokens.index(target)]
        if y==0 or (a==0 and b==0) or (by_target and y<amount):continue
        eligible.append((sid,y))
    name="calculateTradeSourceAmount" if by_target else "calculateTradeTargetAmount"
    sig=name+"(address,address,"+ACTION+")"
    candidates=[]
    for off in range(0,len(eligible),32):
        group=eligible[off:off+32]
        calls=[(market.controller,True,bytes.fromhex(calldata(sig,["address","address",ACTION],[source,target,[(sid,amount)]])[2:])) for sid,_ in group]
        batch,=c.one(MULTI,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[calls],["(bool,bytes)[]"])
        for (sid,y),(ok,data) in zip(group,batch,strict=True):
            if not ok:continue
            other,=decode(["uint128"],data)
            if other and (by_target or other<=y):candidates.append((other,sid))
    refused=[]
    for other,sid in sorted(candidates,reverse=not by_target):
        spent,received=(other,amount) if by_target else (amount,other)
        try:
            result=settle(c,source,target,[(sid,amount)],spent,received,by_target,end,market)
        except InsufficientOrderLiquidity:
            refused.append(dict(id=str(sid),source=source,target=target,amount=str(amount),quoted=str(other),
                                error="InsufficientLiquidity()"))
            continue
        return dict(result,id=str(sid),quote_comparisons=len(candidates),refused=refused)
    return None


def settle(c,source,target,actions,spent,received,by_target,end,market=None):
    """Measure the supplied plan unchanged, including rollback on a tighter limit and expiry."""
    if market is None:market=ETH_MARKET
    def approve(coin,n):
        return tx(coin,"approve(address,uint256)",["address","uint256"],[market.controller,n])
    def getbalance(t):
        return tx(MULTI,"getEthBalance(address)",["address"],[OWNER]) if t==NATIVE else tx(t,"balanceOf(address)",["address"],[OWNER])
    name="tradeByTargetAmount" if by_target else "tradeBySourceAmount"
    def order(limit,expires):
        value=(limit if by_target else spent) if source==NATIVE else 0
        return tx(market.controller,name+"(address,address,"+ACTION+",uint256,uint128)",["address","address",ACTION,"uint256","uint128"],[source,target,actions,expires,limit],value)
    prep=[]
    if source!=NATIVE:
        prep=[tx(market.funding,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[spent,[market.wrapped,source],OWNER,end],10**18),approve(source,spent)]
    balances=[getbalance(source),getbalance(target)]
    limit=spent if by_target else received
    calls=prep+balances+[order(limit-1 if by_target else limit+1,end),order(limit,1)]+balances+[order(limit,end)]+balances
    res=simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(20*10**18)}},calls)
    n=len(prep)
    if all(r["status"]=="0x1" for r in res[:n+2]) and res[n+6].get("error",{}).get("data")=="0xbb55fd27":
        raise InsufficientOrderLiquidity(actions)
    if [i for i,r in enumerate(res) if r["status"]!="0x1"]!=[n+2,n+3]:raise ValueError((actions,source,name,[(i,r.get("error")) for i,r in enumerate(res) if r["status"]!="0x1"]))
    def bal(i):return [int(r["returnData"],16) for r in res[i:i+2]]
    before=bal(n);middle=bal(n+4);after=bal(n+7)
    if before!=middle or [before[0]-after[0],after[1]-before[1]]!=[spent,received]:raise ValueError("Carbon wallet/account delta differs from signed limit")
    return dict(source=source,target=target,by_target=by_target,input=str(spent),output=str(received),approval_gas=int(res[1]["gasUsed"],16) if prep else 0,
                gas=int(res[n+6]["gasUsed"],16),transaction=calls[n+6],logs=res[n+6]["logs"],owner=OWNER,strict=res[n+2]["error"],expired=res[n+3]["error"])

def capture(url):
    tag=dict(blockHash=BLOCK,requireCanonical=True)
    c=Capture(dict(selector=tag,endpoint=url));header=rpc(url,"eth_getBlockByHash",[BLOCK,False])
    if header["stateRoot"]!=ROOT_HASH or rpc(url,"eth_chainId",[])!="0x1":raise ValueError("wrong state")
    slot=hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"),"big")-1)
    role=c.session.post(url,json=dict(jsonrpc="2.0",id=1,method="eth_getStorageAt",params=[CTRL,slot,tag]),timeout=22)
    role.raise_for_status()
    if "0x"+role.json()["result"][-40:]!=IMPL:raise ValueError("unexpected implementation")
    hashes=[digest(bytes.fromhex(rpc(url,"eth_getCode",[a,tag])[2:])) for a in [CTRL,IMPL]]
    if hashes!=["0x928e1f741e909019fa85db08bc2479fdbfdfdab1f16965c44a672fbde5e0ad80","0xcc6f9ba53c3a971839d96b1f632122c47e5eff1624670a562c589e707ce19983"] or c.one(CTRL,"version()",[],[],["uint16"])[0]!=6:raise ValueError("code/version mismatch")
    rows=collect(c); controls=[]; end=int(header["timestamp"],16)+240
    for stable in STABLES:
        row=next(r for r in rows if set(r["tokens"])=={stable,NATIVE})
        for source,target in [(stable,NATIVE),(NATIVE,stable)]:
            for mode in [False,True]:
                for qty in [1000000,5000000] if (target if mode else source)==stable else [10**14,5*10**14]:
                    trial=wallet(c,row,source,target,qty,mode,end)
                    if trial:controls.append(trial)
    if {r["by_target"] for r in controls}!={True,False} or {r["source"]==NATIVE for r in controls}!={True,False}:raise ValueError("incomplete two-sided wallet coverage")
    for row in rows:
        row["orders"]=[[str(sid),owner,tokens,[[str(v) for v in order] for order in orders]]
                       for sid,owner,tokens,orders in row["orders"]]
    return dict(source_revision=REV,chain_id=1,selector=tag,height=int(header["number"],16),state_root=ROOT_HASH,roles=[CTRL,IMPL],hashes=hashes,version=6,directory=rows,controls=controls)

if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("output",type=Path);args=parser.parse_args();errors=[]
    for url in ENDPOINTS:
        try:data=capture(url)
        except (ValueError,RuntimeError,KeyError,requests.RequestException) as e:errors.append((url,str(e)[:350]));continue
        args.output.write_text(json.dumps(data,separators=(",",":"))+"\n",encoding="utf8",newline="\n");break
    else:raise RuntimeError(errors)
