"""Count every HelloV3Pool deployment, verify CREATE2, pool roles and liquidity on one Ethereum block."""
import argparse
import json
from pathlib import Path
import re

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT
from record_ethereum_x7 import ROOT_HASH

FACTORY="0x26aaa70d55fd0988eb9528884c5cd90c95ae0fdf"
DEPLOYER="0xeac47f49eeabdcea711aa3e4f3e0b137a011fdfe"
NAMES=("PoolCreated(address,address,uint24,int24,address)","FeeAmountEnabled(uint24,int24)")
ZERO="0x"+"00"*20

def verified(address,role,chain):
    url="https://sourcify.dev/server/v2/contract/1/"+address
    response=requests.get(url,params={"fields":"sources,compilation,deployment,runtimeBytecode.onchainBytecode"},timeout=32)
    response.raise_for_status(); source=response.json()
    raw=chain("eth_getCode",[address,chain.tag])
    if not (source.get("runtimeMatch") in ("match","exact_match") and source["compilation"]["name"]==role and source["runtimeBytecode"]["onchainBytecode"].lower()==raw.lower()):
        raise ValueError("Ethereum role differs from verified code")
    return dict(address=address,url=url,source_match=source["runtimeMatch"],height=int(source["deployment"]["blockNumber"]),runtime="0x"+keccak(bytes.fromhex(raw[2:])).hex()),source,bytes.fromhex(raw[2:])

def capture(endpoint):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    cap=Capture(dict(snap,endpoint=endpoint))
    def call(method,params):
        if method not in {"eth_getBlockByHash","eth_getBlockByNumber","eth_getCode","eth_getTransactionCount","eth_chainId","eth_getLogs"}:raise ValueError(method)
        response=cap.session.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=25)
        response.raise_for_status()
        result=response.json()
        if "result" not in result:raise ValueError((method,str(result.get("error"))[:200]))
        return result["result"]
    call.tag=cap.selector
    h=call("eth_getBlockByHash",[cap.selector["blockHash"],False])
    if int(call("eth_chainId",[]),16)!=1 or int(h["number"],16)!=snap["block"] or h["stateRoot"]!=ROOT_HASH or h["hash"]!=cap.selector["blockHash"]:raise ValueError("canonical header")
    fac,_,_=verified(FACTORY,"HelloV3Factory",call)
    dep,src,code=verified(DEPLOYER,"HelloV3PoolDeployer",call)
    text=src["sources"]["contracts/HelloV3PoolDeployer.sol"]["content"]
    if "new HelloV3Pool{salt: keccak256(abi.encode(token0, token1, fee))}" not in text:raise ValueError("deployer creation path")
    loader=list(re.finditer(b"\x61(..)\x80\x61(..)\x83\x39\x01\x90\x56\xfe",code,re.S))
    if len(loader)!=1:raise ValueError("ambiguous init code boundary")
    length,offset=(int.from_bytes(i,"big") for i in loader[0].groups())
    init=code[offset:offset+length]
    if offset!=loader[0].end() or len(init)!=length:raise ValueError("deployer codecopy offset")
    init_hash=keccak(init)
    values=posts(cap,[(FACTORY,"poolDeployer()",[],[]),(DEPLOYER,"factoryAddress()",[],[])])
    if [decode(["address"],v)[0] for v in values]!=[DEPLOYER,FACTORY]:raise ValueError("factory and deployer mismatch")
    topics=["0x"+keccak(text=n).hex() for n in NAMES]
    logs=call("eth_getLogs",[dict(address=FACTORY,fromBlock=hex(fac["height"]),toBlock=h["number"],topics=[topics])])
    headers={};tiers={};markets=[];ids=set()
    for row in logs:
        if row.get("removed") or row["address"].lower()!=FACTORY:raise ValueError("unexpected emitter")
        bn=row["blockNumber"]; idx=(bn,row["logIndex"])
        if idx in ids:raise ValueError("duplicate log")
        ids.add(idx)
        if bn not in headers:headers[bn]=call("eth_getBlockByNumber",[bn,False])["hash"]
        if headers[bn]!=row["blockHash"]:raise ValueError("event is not canonical")
        t=row["topics"]
        if t[0]==topics[1]:
            fee=int(t[1],16); spacing=decode(["int24"],bytes.fromhex(t[2][2:]))[0]
            if fee in tiers:raise ValueError("duplicate fee tier")
            tiers[fee]=spacing
            continue
        tokens=["0x"+x[-40:] for x in t[1:3]];fee=int(t[3],16)
        step,addr=decode(["int24","address"],bytes.fromhex(row["data"][2:]))
        salt=keccak(encode(["address","address","uint24"],[*tokens,fee]))
        derived="0x"+keccak(b"\xff"+bytes.fromhex(DEPLOYER[2:])+salt+init_hash)[-20:].hex()
        if addr!=derived or sorted(tokens)!=tokens or tiers.get(fee)!=step:raise ValueError("wrong market identity")
        markets.append(dict(pool=addr,tokens=tokens,fee=fee,step=step,block=int(bn,16),transaction=row["transactionHash"]))
    count=int(call("eth_getTransactionCount",[DEPLOYER,cap.selector]),16)
    if len(markets)+1!=count or len({m["pool"] for m in markets})!=len(markets):raise ValueError("incomplete deployment directory")
    methods=("factory()","token0()","token1()","fee()","tickSpacing()","liquidity()","lmPool()","slot0()")
    fields=posts(cap,[(m["pool"],s,[],[]) for m in markets for s in methods])
    duplicates=posts(cap,[(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[*t,m["fee"]]) for m in markets for t in [m["tokens"],m["tokens"][::-1]]])
    for i,m in enumerate(markets):
        v=fields[i*len(methods):(i+1)*len(methods)]
        roles=[decode(["address"],x)[0] for x in v[:3]]
        if roles!=[FACTORY,*m["tokens"]] or m["fee"]!=int.from_bytes(v[3],"big") or m["step"]!=decode(["int24"],v[4])[0] or any(decode(["address"],x)[0]!=m["pool"] for x in duplicates[i*2:i*2+2]):raise ValueError("pool getters disagree")
        m["liquidity"]=str(decode(["uint128"],v[5])[0]);m["lm_pool"]=decode(["address"],v[6])[0]
        slot=decode(["uint160","int24","uint16","uint16","uint16","uint32","bool"],v[7])
        m["slot0"]=[str(slot[0]),*slot[1:]]
    if call("eth_getBlockByNumber",[h["number"],False])["hash"]!=h["hash"]:raise ValueError("orphaned registry")
    return dict(chain="ethereum",chain_id=1,selector=cap.selector,state_root=ROOT_HASH,height=snap["block"],factory=FACTORY,deployer=DEPLOYER,roles=[fac,dep],
                nonce=count,init_hash="0x"+init_hash.hex(),codecopy=[offset,length],tiers=tiers,markets=markets,events=logs)

if __name__=="__main__":
    args=argparse.ArgumentParser(description=__doc__);args.add_argument("output",type=Path);args.add_argument("--endpoint",default="https://mainnet.gateway.tenderly.co");cfg=args.parse_args()
    cfg.output.write_text(json.dumps(capture(cfg.endpoint),indent=2)+"\n",encoding="utf8",newline="\n")
