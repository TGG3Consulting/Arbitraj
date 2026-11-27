"""Maverick V2: полный реестр #4752, направленные комиссии и реальные PoolSwap на каноне Base/Ethereum."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT
from record_llama_top_contracts import rpc

DATA=ROOT/"tests/data/first_run_dex_2026-11-26"
FACTORY="0x0a7e848aca42d879ef06507fca0e7b33a0a63c1e"
LENS="0x6a9eb38de5d349fe751e0adb4c0d9d391f94cc8d"
QUOTER="0xb40afdb85a07f37ae217e7d6462e609900dd8d7a"
ROUTERS={"base":"0x5eded0d7e76c563ff081ca01d9d12d6b404df527","ethereum":"0x62e31802c6145a2d5e842eed8efe01fc224422fa"}
ENDPOINTS={"base":"https://base.rpc.blxrbdn.com","ethereum":"https://eth.drpc.org"}
STATE="(uint128,uint128,int64,int64,uint40,int32,bool,uint32,uint8)"
TOPIC="0x"+keccak(text="PoolSwap(address,address,(uint256,bool,bool,int32),uint256,uint256)").hex()
SRC="https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/maverick-v2/index.ts"
DOC="https://docs.mav.xyz/technical-reference/contract-addresses/v2-contract-addresses"


def capture(network):
    snapshot=json.loads((DATA/(network+".json")).read_bytes())
    height,at=snapshot["block"],snapshot["selector"]
    c=Capture(dict(snapshot,endpoint=ENDPOINTS[network]))
    block=lambda n:rpc(network,"eth_getBlockByNumber",[hex(n),False])
    header=block(height)
    assert header["hash"]==at["blockHash"] and int(rpc(network,"eth_chainId",[]),16)==snapshot["chain_id"]
    def role(a):
        code=bytes.fromhex(rpc(network,"eth_getCode",[a,at])[2:])
        if not code:raise ValueError("missing role "+a)
        return dict(address=a,bytes=len(code),runtime="0x"+keccak(code).hex())
    roles={k:role(a) for k,a in [("factory",FACTORY),("quoter",QUOTER),("lens",LENS),("router",ROUTERS[network])]}
    roles["router"]["wrapped"],=c.one(ROUTERS[network],"weth()",returns=["address"])
    actual,=c.one(ROUTERS[network],"factory()",returns=["address"])
    if actual!=FACTORY:raise ValueError("different router factory")
    total,=c.one(FACTORY,"poolCount()",returns=["uint256"])
    pools,=c.one(FACTORY,"lookup(uint256,uint256)",["uint256","uint256"],[0,total],["address[]"])
    if len(pools)!=len(set(pools)) or len(pools)!=total:raise ValueError("incomplete directory")
    queries=[];outs=[]
    for pool in pools:
        for target,sig,ins,vals,typ in [(FACTORY,"isFactoryPool(address)",["address"],[pool],"bool"),
            (pool,"factory()",[],[],"address"),(pool,"tokenA()",[],[],"address"),(pool,"tokenB()",[],[],"address"),
            (pool,"fee(bool)",["bool"],[True],"uint256"),(pool,"fee(bool)",["bool"],[False],"uint256"),
            (pool,"getState()",[],[],STATE),(pool,"tickSpacing()",[],[],"uint256"),
            (pool,"accessor()",[],[],"address"),(pool,"permissionedSwap()",[],[],"bool"),
            (pool,"permissionedLiquidity()",[],[],"bool"),(pool,"permissionedPool()",[],[],"bool")]:
            queries.append((target,sig,ins,vals));outs.append(typ)
    values=iter(decode([t],raw)[0] for raw,t in zip(posts(c,queries),outs,strict=True))
    markets={}
    for pool in pools:
        valid,factory,a,b,fa,fb,st,spacing,access,ps,pl,pp=(next(values) for _ in range(12))
        if not valid or factory!=FACTORY or a>=b:raise ValueError("pool roles differ: "+pool)
        markets[pool]=dict(tokens=[a,b],fees_d18=[str(fa),str(fb)],reserves=list(map(str,st[:2])),active_tick=st[5],
            locked=st[6],bin_count=st[7],protocol_ratio_d3=st[8],tick_spacing=spacing,accessor=access,
            permissioned_swap=ps,permissioned_liquidity=pl,permissioned_pool=pp)
    cutoff=int(header["timestamp"],16)-86400
    lo=max(0,height-600000);hi=height
    if int(block(lo)["timestamp"],16)>=cutoff:raise ValueError("24 hour boundary unavailable")
    while lo+1<hi:
        mid=(lo+hi)//2
        if int(block(mid)["timestamp"],16)<cutoff:lo=mid
        else:hi=mid
    start=hi
    events={}
    session=requests.Session()
    groups=[pools] if network=="base" else [pools[i:i+10] for i in range(0,total,10)]
    step=64000 if network=="base" else 6000
    for group in groups:
        for h in range(start,height+1,step):
            params=[dict(address=group,topics=[TOPIC],fromBlock=hex(h),toBlock=hex(min(h+step-1,height)))]
            if network=="base":
                r=session.post("https://base.rpc.blxrbdn.com",json={"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":params},timeout=30)
                r.raise_for_status();reply=r.json()
                if "error" in reply:raise ValueError(str(reply["error"])[:180])
                logs=reply["result"]
            else:
                logs=rpc(network,"eth_getLogs",params)
            for log in logs:
                pool=log["address"].lower()
                if log.get("removed") or pool not in markets or log["topics"]!=[TOPIC]:raise ValueError("bad PoolSwap")
                sender,recipient,(specified,a_in,exact_out,limit),debit,credit=decode(
                    ["address","address","(uint256,bool,bool,int32)","uint256","uint256"],bytes.fromhex(log["data"][2:]))
                side="A" if a_in else "B"
                item=events.setdefault(pool,{}).setdefault(side,{"count":0,"paid":0,"received":0,"partial":0})
                item["count"]+=1;item["paid"]+=debit;item["received"]+=credit
                item["partial"]+=int((credit if exact_out else debit)<specified)
                item["sample"]={k:log[k] for k in ("blockHash","blockNumber","transactionHash","logIndex","topics","data")}
    if block(height)["hash"]!=at["blockHash"]:raise ValueError("anchor changed")
    return dict(source=SRC,official=DOC,chain_id=snapshot["chain_id"],height=height,selector=at,state_root=header["stateRoot"],
                first_block=start,cutoff_unix=cutoff,roles=roles,markets=markets,events=events)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("network",choices=ENDPOINTS)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    capture_bytes=json.dumps(capture(args.network),separators=(",",":")).encode()
    args.output.write_bytes(gzip.compress(capture_bytes,mtime=0) if args.output.suffix==".gz" else capture_bytes)
