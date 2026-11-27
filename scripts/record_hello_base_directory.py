"""Проверяет все рынки HelloV3Factory посредством nonce отдельного deployer и событий PoolCreated."""
import argparse
import json
from pathlib import Path
import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_balancer_markets import Capture,posts
from record_base_native_v3_directory import CanonicalReads,directory
from record_base_slipstream_executor import ROOT,rpc
from record_hello_ethereum_directory import DEPLOYER as ETH_DEPLOYER

DATA=ROOT/"tests/data/first_run_dex_2026-11-26"
FACTORY="0x044963d0ea07accf3c4349a72811a42acc2369d1"
DEPLOYER="0x9fbc2843fb0e7b416fc42e2f800ad91df96ecdb7"

def record():
    snap=json.loads((DATA/"base.json").read_bytes())
    eth=json.loads((DATA/"hello_ethereum_directory.json").read_bytes())
    other=json.loads((DATA/"base_native_v3.json").read_bytes())
    if other["selector"]!=snap["selector"]:raise ValueError("источник другой высоты")
    read=CanonicalReads(other); c=Capture(dict(snap,endpoint=read.nodes[0]))
    root="https://sourcify.dev/server/v2/contract/"
    roles=[]
    for expected,actual,name,source_chain in [(FACTORY,FACTORY,"HelloV3Factory","8453"),(DEPLOYER,ETH_DEPLOYER,"HelloV3PoolDeployer","1")]:
        u=root+source_chain+"/"+actual
        r=requests.get(u,params={"fields":"runtimeBytecode.onchainBytecode,compilation,deployment"},timeout=30);r.raise_for_status();src=r.json()
        code,=read.read([("eth_getCode",[expected,c.selector])]); code=bytes.fromhex(code[2:])
        if (src.get("runtimeMatch") not in ("match","exact_match") or src["compilation"]["name"]!=name or
                bytes.fromhex(src["runtimeBytecode"]["onchainBytecode"][2:])!=code):raise ValueError("код роли HELLO изменился")
        roles.append(dict(address=expected,url=u,equivalent_address=actual,runtime="0x"+keccak(code).hex()))
        if expected==FACTORY: birth=int(src["deployment"]["blockNumber"])
        else:
            offset,length=eth["codecopy"]
            init=keccak(code[offset:offset+length])
            if "0x"+init.hex()!=eth["init_hash"]:raise ValueError("другой код создания")
    n=int(read.read([("eth_getTransactionCount",[DEPLOYER,c.selector])])[0],16)
    if c.one(FACTORY,"poolDeployer()",returns=["address"])[0]!=DEPLOYER or c.one(DEPLOYER,"factoryAddress()",returns=["address"])[0]!=FACTORY:raise ValueError("несовпадение ролей")
    markets=directory(read,other,DEPLOYER,FACTORY,birth,n-1)
    methods=["factory()","token0()","token1()","fee()","tickSpacing()","liquidity()","lmPool()","slot0()"]
    fields=posts(c,[(m["pool"],method,[],[]) for m in markets for method in methods])
    for i,m in enumerate(markets):
        fields_i=fields[i*len(methods):(i+1)*len(methods)]
        t0,t1=m["tokens"];m["step"]=m.pop("spacing");fee=m["fee"]
        addr="0x"+keccak(b"\xff"+bytes.fromhex(DEPLOYER[2:])+keccak(encode(["address","address","uint24"],[t0,t1,fee]))+init)[-20:].hex()
        expected=[FACTORY,t0,t1,fee,m["step"]]
        vals=[decode([t],raw)[0] for t,raw in zip(["address","address","address","uint24","int24","uint128","address"],fields_i)]
        if addr!=m["pool"] or expected!=vals[:5]:raise ValueError("CREATE2 либо состояние пула расходятся")
        m["liquidity"]=str(vals[5]);m["lm_pool"]=vals[6]
        slot=decode(["uint160","int24","uint16","uint16","uint16","uint32","bool"],fields_i[7]);m["slot0"]=[str(slot[0]),*slot[1:]]
    tier_set={r["fee"] for r in markets}.union(int(k) for k in eth["tiers"])
    ticks=posts(c,[(FACTORY,"feeAmountTickSpacing(uint24)",["uint24"],[fee]) for fee in sorted(tier_set)])
    tiers={str(fee):space for fee,b in zip(sorted(tier_set),ticks,strict=True) if (space:=decode(["int24"],b)[0])>0}
    return dict(chain="base",chain_id=8453,selector=c.selector,height=snap["block"],state_root=other["state_root"],roles=roles,
                factory=FACTORY,deployer=DEPLOYER,init_hash="0x"+init.hex(),codecopy=eth["codecopy"],tiers=tiers,nonce=n,markets=markets)

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);args=ap.parse_args()
    args.output.write_text(json.dumps(record(),indent=2)+"\n",encoding="utf8",newline="\n")
