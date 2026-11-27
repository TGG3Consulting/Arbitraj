"""Каталог Goblins AMM Base. Nonce фабрики и корень хранилища проверяют полноту."""
import json
from pathlib import Path
import requests
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT, calldata
from v3_factory_storage import prove_account, verify_directory

FACTORY = "0xe82fa4d4ff25bad8b07c4d1ebd50e83180dd5eb8"
QUOTER = "0xcbf872c837bad15b0a0283a5c5936fbd1b66beaf"
ROUTER = "0xe29a829a1c86516b1d24b7966af14eb1be2cd5d4"
DEPLOY = 21481309
DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
NODES = ["https://base.drpc.org","https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"]
POOL = "0x"+keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
FEE = "0x"+keccak(text="FeeAmountEnabled(uint24,int24)").hex()
ALLOWED = {"eth_getCode","eth_getTransactionCount","eth_getLogs","eth_call","eth_getBlockByHash","eth_getProof","eth_chainId"}

def batch(calls):
    if any(m not in ALLOWED for m,_ in calls):raise ValueError("unexpected RPC method")
    if not calls:return []
    if len(calls)>12:
        return [row for i in range(0,len(calls),12) for row in batch(calls[i:i+12])]
    q=[dict(jsonrpc="2.0",id=i,method=m,params=p) for i,(m,p) in enumerate(calls)]
    for site in NODES:
        try:
            response=requests.post(site,json=q,timeout=15)
            response.raise_for_status()
            rows=sorted(response.json(),key=lambda v:v["id"])
            if len(rows)==len(q) and all(row["id"]==i and row.get("result") is not None for i,row in enumerate(rows)):
                return [row["result"] for row in rows]
        except (ValueError,KeyError,TypeError,requests.RequestException):
            pass
    if len(calls)>1:
        mid=len(calls)//2;return batch(calls[:mid])+batch(calls[mid:])
    raise ValueError(("all archive endpoints rejected",calls[0][0],str(calls[0][1])[:110]))

def single(m,p):return batch([(m,p)])[0]

def read(a,func,args=(),values=(),tag=None):
    return ("eth_call",[{"to":a,"data":calldata(func,args,values)},tag])

def discover():
    snap=json.loads((DATA/"base.json").read_bytes());h=snap["selector"];end=snap["block"]
    header=single("eth_getBlockByHash",[h["blockHash"],False]);assert int(header["number"],16)==end
    proof=single("eth_getProof",[FACTORY,[],h]);storage=prove_account(header,proof,FACTORY)
    hi=int(proof["nonce"],16);counts={DEPLOY-1:0,end:hi};changed={}
    intervals=[(DEPLOY,end)]
    counts[DEPLOY]=int(single("eth_getTransactionCount",[FACTORY,hex(DEPLOY)]),16)
    while intervals:
        nxt=[]; mids=[]
        for l,r in intervals:
            if counts[l]==counts[r]:continue
            if l+1==r:
                changed[r]=counts[r]-counts[l]
            else:
                m=(l+r)//2;mids.append(m);nxt.extend([(l,m),(m,r)])
        counts.update({m:int(v,16) for m,v in zip(mids,batch([("eth_getTransactionCount",[FACTORY,hex(m)]) for m in mids]),strict=True)})
        intervals=nxt
    records=batch([("eth_getLogs",[dict(address=FACTORY,fromBlock=hex(b),toBlock=hex(b),topics=[[FEE,POOL]])]) for b in [DEPLOY,*sorted(changed)]])
    markets=[];fees={};fee_logs=[]
    for b,logs in zip([DEPLOY,*sorted(changed)],records,strict=True):
        num=0
        for log in logs:
            ts=log["topics"];assert int(log["blockNumber"],16)==b and log["address"].lower()==FACTORY
            if ts[0]==POOL:
                num+=1;gap,pool=decode(["int24","address"],bytes.fromhex(log["data"][2:]))
                markets.append(dict(pool=pool,tokens=["0x"+v[-40:] for v in ts[1:3]],fee=int(ts[3],16),spacing=gap,
                                    block=b,hash=log["blockHash"],tx=log["transactionHash"]))
            else:
                fee=int(ts[1],16);gap,=decode(["int24"],bytes.fromhex(ts[2][2:]));fees[str(fee)]=gap;fee_logs.append(log)
        assert num==changed.get(b,counts[DEPLOY]-1)
    # Any further, unused registration is visible in the storage root. Resolve the
    # observed pool tiers independently at the canonical head before reconstruction.
    pending=sorted(({m["fee"] for m in markets} | {1,10,50,100,200,250,300,400,1000,2000,2500,5000}) - set(map(int,fees)))
    for fee,raw in zip(pending,batch([read(FACTORY,"feeAmountTickSpacing(uint24)",["uint24"],[f],h) for f in pending]),strict=True):
        gap,=decode(["int24"],bytes.fromhex(raw[2:]))
        if gap:fees[str(fee)]=gap
    owner,=batch([read(FACTORY,"owner()",tag=h)])
    data=dict(factory=FACTORY,selector=h,from_block=DEPLOY,to=end,state_root=header["stateRoot"],account_proof=proof["accountProof"],
              storage_root="0x"+storage.hex(),nonce=hi,owner=owner,layout=dict(owner=3,feeAmountTickSpacing=4,getPool=5),
              storage_slots=len(markets)*2+len(fees)+bool(int(owner,16)),tiers=fees,pools=markets,fee_events=fee_logs)
    verify_directory(data,FACTORY)
    queries=[]
    for m in markets:
        a,b=m["tokens"];p=m["pool"]
        queries.extend([read(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[b,a,m["fee"]],h),
             read(p,"liquidity()",tag=h),read(a,"balanceOf(address)",["address"],[p],h),read(b,"balanceOf(address)",["address"],[p],h)])
    vals=batch(queries)
    for i,m in enumerate(markets):
        pool,liq,a,b=vals[4*i:4*i+4];assert "0x"+pool[-40:]==m["pool"]
        m.update(liquidity=str(int(liq,16)),balances=[str(int(a,16)),str(int(b,16))])
    return data

if __name__=="__main__":
    import argparse
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output",type=Path);args=cli.parse_args()
    args.output.write_text(json.dumps(discover(),indent=2)+"\n",encoding="utf8")
