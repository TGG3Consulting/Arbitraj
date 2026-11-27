"""Enumerate both Antfarm pair bytecodes and record fees and inventory on one block."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import rpc
from record_carbon_ethereum import BLOCK, ENDPOINTS, ROOT_HASH
from record_ethereum_v2_factories import WETH
from record_ethereum_x7 import verified

FACTORY="0xe48aee124f9933661d4dd3eb265fa9e153e32cbe"
ROUTER="0x6d9f0eb21d77c6d24be49a579508471e937d5418"
ATF="0x518b63da813d46556fea041a88b52e3caa8c16a8"
INIT=["2f47d72b208014a5ba4f32371ac96dd421a39152dcaf104e8232b6c9f1a92280",
      "b174de46ec9038ead3d74ed04c79d4885d8e642175833c4da037d5e052492e5b"]
DOC="https://docs.antfarm.finance/contracts/antfarm-contracts/ethereum"
ZERO="0x"+"00"*20


def raw(address):
    return bytes.fromhex(address[2:])


def capture(c):
    header=rpc(c.endpoint,"eth_getBlockByHash",[BLOCK,False])
    if rpc(c.endpoint,"eth_chainId",[])!="0x1" or header["stateRoot"]!=ROOT_HASH:
        raise ValueError("wrong Antfarm root")
    roles={}
    for address,name in ((FACTORY,"AntfarmFactory"),(ROUTER,"AntfarmRouter")):
        url,_,code=verified(c,address,name)
        roles[name]=dict(address=address,hash="0x"+keccak(code).hex(),source=url)
    identity=[(a,k,[],[]) for a,k in [(FACTORY,"antfarmToken()"),(ROUTER,"factory()"),
              (ROUTER,"antfarmToken()"),(ROUTER,"WETH()")]]
    if [decode(["address"],x)[0] for x in posts(c,identity)]!=[ATF,FACTORY,ATF,WETH]:
        raise ValueError("Antfarm router binding")
    total,=c.one(FACTORY,"allPairsLength()",returns=["uint256"])
    pairs=[]
    for start in range(0,total,64):
        page,end=c.one(FACTORY,"getPairs(uint256,uint256)",["uint256","uint256"],[start,64],["address[]","uint256"])
        if end!=start+len(page):raise ValueError("invalid directory cursor")
        pairs.extend(page)
    if len(set(pairs))!=total or len(pairs)!=total:raise ValueError("incomplete directory")
    cols=[("factory()",["address"]),("token0()",["address"]),("token1()",["address"]),
          ("fee()",["uint16"]),("getReserves()",["uint112","uint112","uint32"]),("totalSupply()",["uint256"]),
          ("antfarmTokenReserve()",["uint256"]),("antfarmOracle()",["address"])]
    vals=iter(posts(c,[(p,k,[],[]) for p in pairs for k,_ in cols]))
    rows=[]; checks=[]
    for p in pairs:
        fac,a,b,fee,res,supply,escrow,oracle=[decode(types,next(vals)) for _,types in cols]
        a,b,fee=a[0],b[0],fee[0]
        if fac[0]!=FACTORY or a==b or (ATF not in (a,b) and a>=b) or b==ATF:raise ValueError("token orientation")
        salt=keccak(raw(a)+raw(b)+fee.to_bytes(2,"big")+raw(ATF))
        derived="0x"+keccak(b"\xff"+raw(FACTORY)+salt+bytes.fromhex(INIT[a==ATF]))[-20:].hex()
        if derived!=p:raise ValueError("pool creation hash")
        for src,dst in ((a,b),(b,a)):
            checks.append((FACTORY,"getPair(address,address,uint16)",["address","address","uint16"],[src,dst,fee]))
        checks.extend((t,"balanceOf(address)",["address"],[p]) for t in (a,b,ATF))
        rows.append(dict(pair=p,tokens=[a,b],fee=fee,reserves=list(map(str,res[:2])),supply=str(supply[0]),
                         escrow=str(escrow[0]),oracle=oracle[0],pair_class="atf" if a==ATF else "external_fee"))
    codes=(rpc(c.endpoint,"eth_getCode",[p,c.selector]) for p in pairs)
    for row,code in zip(rows,codes,strict=True):
        if code=="0x":raise ValueError("registered Antfarm pair missing code")
        row["runtime_hash"]="0x"+keccak(hexstr=code).hex()
    vals=iter(posts(c,checks))
    for row in rows:
        if [decode(["address"],next(vals))[0] for _ in range(2)]!=[row["pair"]]*2:raise ValueError("registry mismatch")
        row["balances"]=[str(int.from_bytes(next(vals),"big")) for _ in range(3)]
    oracles=sorted({r["oracle"] for r in rows if r["oracle"]!=ZERO})
    keys=[("pair()",["address"]),("token1()",["address"]),("price1Average()",["uint224"]),("firstUpdateCall()",["bool"])]
    vals=iter(posts(c,[(o,k,[],[]) for o in oracles for k,_ in keys]))
    sources={}
    for o in oracles:
        pool,coin,rate,pending=[decode(t,next(vals))[0] for _,t in keys]
        if not any(r["pair"]==pool and r["fee"]==10 and r["tokens"]==[ATF,coin] and r["oracle"]==o for r in rows):
            raise ValueError("price source does not belong to a 1% ATF pool")
        sources[o]=dict(pair=pool,asset=coin,rate_q112=str(rate),first_update=pending)
    return dict(selector=c.selector,state_root=ROOT_HASH,height=int(header["number"],16),contracts=roles,official=DOC,
                block_time=int(header["timestamp"],16),fee_token=ATF,init_hashes=INIT,pairs=rows,oracles=sources)


if __name__=="__main__":
    arg=argparse.ArgumentParser();arg.add_argument("output",type=Path);args=arg.parse_args()
    for url in ENDPOINTS:
        try:
            result=capture(Capture(dict(selector=dict(blockHash=BLOCK,requireCanonical=True),endpoint=url)))
        except (ValueError,RuntimeError,KeyError,requests.RequestException) as exc:
            print(url,str(exc)[:300],flush=True)
            continue
        args.output.write_text(json.dumps(result,separators=(",",":"))+"\n",encoding="utf8")
        break
    else:raise RuntimeError("Antfarm canonical registry unavailable")
