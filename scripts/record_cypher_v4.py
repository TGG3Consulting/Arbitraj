"""Реестр Cypher Algebra Integral: штатные и custom-пулы на одном каноническом хеше Ethereum."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
import requests

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import rpc
from record_cypher_degen_v2 import DATA, source
from record_ethereum_v2_factories import digest, raw
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

FACTORY="0xfb8ed3485efa29a0e4bed93351dd51b59fc4b0f0"
QUOTER="0x02f22d58d161d1c291abfe88764d84120f20f723"
ROUTER="0x20c5893f69f635f55b0367c519f3f95e59c0b0ab"
ZERO="0x"+"00"*20
POOL=digest(b"Pool(address,address,address)")
CUSTOM=digest(b"CustomPool(address,address,address,address)")

def plugin_template(code,pool):
    # Меняем только полный операнд PUSH20/PUSH32, равный адресу этой же пары.
    out=bytearray(code);i=0;address=bytes.fromhex(pool[2:]);references=[]
    while i<len(out):
        opcode=out[i];size=opcode-0x5f if 0x60<=opcode<=0x7f else 0
        if size in (20,32) and out[i+1:i+1+size]==address.rjust(size,b"\0"):
            out[i+1:i+1+size]=bytes(size);references.append(i+1)
        i+=1+size
    return digest(out),references

def exact_assets(c):
    assets={}
    for symbol,a,snippets in (
            ("LINK","0x514910771af9ca656af840dff83e8264ecf986ca",
             ("balances[msg.sender] = balances[msg.sender].sub(_value)","balances[_from] = balances[_from].sub(_value)",
              "balances[_to] = balances[_to].add(_value)","return super.transferFrom(_from, _to, _value)")),
            ("wstETH","0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0",
             ('_balances[sender] = _balances[sender].sub(amount, "ERC20: transfer amount exceeds balance")',
              "_balances[recipient] = _balances[recipient].add(amount)","emit Transfer(sender, recipient, amount)"))):
        proof,code,bodies=source(c,a)
        if not all(needle in bodies for needle in snippets):raise ValueError("changed ERC20 transfer: "+symbol)
        assets[symbol]=dict(proof,bytes=len(code),transfer_source=list(snippets))
    return assets

def capture(ep):
    snap=json.loads((DATA/"ethereum.json").read_bytes()); c=Capture(dict(snap,endpoint=ep))
    tag=c.selector;head=rpc(ep,"eth_getBlockByHash",[tag["blockHash"],False]);height=int(head["number"],16)
    if head["stateRoot"]!=ROOT_HASH or head["hash"]!=tag["blockHash"] or height!=snap["block"] or rpc(ep,"eth_chainId",[])!="0x1":raise ValueError("wrong header")
    f,_,_=source(c,FACTORY);q,_,_=source(c,QUOTER);r,_,_=source(c,ROUTER)
    physical,=c.one(FACTORY,"poolDeployer()",returns=["address"])
    init,=c.one(FACTORY,"POOL_INIT_CODE_HASH()",returns=["bytes32"])
    bindings=posts(c,[(a,name,[],[]) for a in [QUOTER,ROUTER] for name in ["factory()","poolDeployer()"]])
    if [decode(["address"],v)[0] for v in bindings]!=[FACTORY,physical]*2:raise ValueError("quoter/SwapRouter factory or pool deployer mismatch")
    codelink,_,_=source(c,physical)
    req=dict(address=FACTORY,fromBlock="0x0",toBlock=hex(height),topics=[[POOL,CUSTOM]])
    response=c.session.post(ep,json=dict(jsonrpc="2.0",id=1,method="eth_getLogs",params=[req]),timeout=45)
    response.raise_for_status();logs=response.json().get("result")
    if not isinstance(logs,list) or not logs:raise ValueError("factory history unavailable")
    named={};markets=[]
    for l in logs:
        if l["address"].lower()!=FACTORY or l.get("removed") or int(l["blockNumber"],16)>height:raise ValueError("wrong factory record")
        if l["topics"][0]==POOL and len(l["topics"])==3: deployer=ZERO
        elif l["topics"][0]==CUSTOM and len(l["topics"])==4:deployer="0x"+l["topics"][1][-40:]
        else:raise ValueError("wrong event")
        a,b=["0x"+v[-40:] for v in l["topics"][-2:]];pool=decode(["address"],raw(l["data"]))[0]
        if a>=b or pool in named:raise ValueError("unsorted or duplicate market")
        named[pool]=True;markets.append(dict(pool=pool,tokens=[a,b],deployer=deployer,registration=l))
    checks=[]
    for m in markets:
        a,b=m["tokens"];dep=m["deployer"];pool=m["pool"]
        types=["address","address"] if dep==ZERO else ["address"]*3
        args=[a,b] if dep==ZERO else [dep,a,b]
        calls=["poolByPair(address,address)","computePoolAddress(address,address)"] if dep==ZERO else [
               "customPoolByPair(address,address,address)","computeCustomPoolAddress(address,address,address)"]
        checks.extend([(FACTORY,s,types,args) for s in calls])
        checks.extend([(pool,k,[],[]) for k in ("token0()","token1()","factory()","liquidity()","tickSpacing()","plugin()","globalState()")])
    vals=iter(posts(c,checks))
    plug=set()
    for m in markets:
        get_addr=lambda:decode(["address"],next(vals))[0]
        if [get_addr() for _ in range(5)]!=[m["pool"],m["pool"],*m["tokens"],FACTORY]:raise ValueError("canonical registry differs")
        liq=decode(["uint128"],next(vals))[0]; spacing=decode(["int24"],next(vals))[0];plugin=get_addr()
        price,tick,fee,config,community,unlocked=decode(["uint160","int24","uint16","uint8","uint16","bool"],next(vals))
        m.update(liquidity=str(liq),spacing=spacing,plugin=plugin,price=str(price),tick=tick,last_fee=fee,plugin_config=config,community_fee=community,unlocked=unlocked)
        if plugin!=ZERO:plug.add(plugin)
    # Bytecode and lookup on the same parent, collecting rather than guessing plugin generations.
    codes={}
    for a in sorted(plug):
        b=raw(rpc(ep,"eth_getCode",[a,tag]))
        if not b:raise ValueError("missing plugin code")
        pools=[m["pool"] for m in markets if m["plugin"]==a]
        template,offsets=plugin_template(b,pools[0]) if len(pools)==1 else (digest(b),[])
        codes[a]=dict(runtime=digest(b),bytes=len(b),template=template,pool_operands=offsets)
    return dict(height=height,selector=tag,state_root=ROOT_HASH,source="https://docs.cyphereth.com/docs/contracts/mainnet/",
                factory=f,router=r,quoter=q,physical=codelink,exact_assets=exact_assets(c),init="0x"+init.hex(),plugins=codes,markets=markets)

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);args=parser.parse_args()
    for endpoint in ENDPOINTS:
        try:
            d=capture(endpoint);b=(json.dumps(d,separators=(",",":"))+"\n").encode()
            args.output.write_bytes(gzip.compress(b,mtime=0) if args.output.suffix==".gz" else b);break
        except (requests.RequestException,KeyError,RuntimeError,ValueError) as exc:print(endpoint,str(exc)[:220],flush=True)
    else:raise ValueError("no canonical factory registry")
