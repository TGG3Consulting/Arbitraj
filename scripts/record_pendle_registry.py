"""Реестр Ethereum Pendle на каноническом блоке. Разделяет физические PT/SY/YT, активы депозитов/погашений и состояние конкретного рынка."""
import argparse
from collections import Counter
from pathlib import Path
import json
import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_bitswap_ethereum import BLOCK, ENDPOINTS
from record_ethereum_x7 import ROOT_HASH

SOURCE = "https://github.com/pendle-finance/pendle-core-v2-public/blob/82e69f0b2279c85f250a87cb0fe16a2b432ebb72/deployments/1-core.json"
FACTORIES = {
    "marketFactory": "0x27b1dacd74688af24a64bd3c9c1b143118740784",
    "marketFactoryV3": "0x1a6fcc85557bc4fb7b534ed835a03ef056552d52",
    "marketFactoryV4": "0x3d75bd20c983edb5fd218a1b7e0024f1056c7a2f",
    "marketFactoryV5": "0x6fcf753f2c67b83f7b09746bbc4fa0047b35d050",
    "marketFactoryV6": "0x6d247b1c044fa1e22e6b04fa9f71baf99eb29a9f",
}
ROUTER="0x888888888889758f76e7103c6cbf23abbf58f946"
STATIC="0x263833d47ea3fa4a30f269323aba6a107f9eb14c"
ZERO="0x"+"00"*20
MULTICALL="0xca11bde05977b3631167028862be2a173976ca11"
SLOT="0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
TOPICS=["0x"+keccak(text="CreateNewMarket(address,address,int256,int256"+suffix+")").hex() for suffix in ("",",uint256")]
AT={"blockHash":BLOCK,"requireCanonical":True}
STATE=["int256","int256","int256","address","int256","uint256","uint256","uint256","uint256"]

def rpc(session,url,method,params):
    r=session.post(url,json={"id":1,"jsonrpc":"2.0","method":method,"params":params},timeout=40)
    r.raise_for_status();v=r.json()
    if "error" in v:raise ValueError((method,v["error"]))
    return v["result"]

def call(to,sig,outputs,args=(),values=()):
    return to,keccak(text=sig)[:4]+encode(args,values),outputs

def read(session,url,rows,tag=AT):
    return read_many(rows,tag,lambda m,p:rpc(session,url,m,p))


def read_many(rows,tag,fetch):
    result=[]
    for at in range(0,len(rows),96):
        part=rows[at:at+96]
        data=keccak(text="aggregate3((address,bool,bytes)[])")[:4]+encode(["(address,bool,bytes)[]"],[[(addr,True,wire)for addr,wire,_ in part]])
        out=fetch("eth_call",[{"to":MULTICALL,"data":"0x"+data.hex()},tag])
        raw=decode(["(bool,bytes)[]"],bytes.fromhex(out[2:]))[0]
        if len(part)!=len(raw):raise ValueError("incomplete multicall")
        for (_,_,abi),(ok,values) in zip(part,raw,strict=True):
            if not ok or not values:result.append(None);continue
            try:result.append(list(decode(abi,values)))
            except (ValueError,OverflowError):result.append(None)
    return result

def capture(url):
    session=requests.Session()
    head=rpc(session,url,"eth_getBlockByHash",[BLOCK,False])
    if rpc(session,url,"eth_chainId",[])!="0x1" or head["stateRoot"]!=ROOT_HASH:raise ValueError("другая сеть или высота")
    if rpc(session,url,"eth_getBlockByNumber",[head["number"],False])["hash"]!=BLOCK:raise ValueError("другой канонический блок")
    logs=rpc(session,url,"eth_getLogs",[{"fromBlock":"0x0","toBlock":head["number"],"address":list(FACTORIES.values()),"topics":[TOPICS]}])
    return snapshot(head,AT,FACTORIES,ROUTER,STATIC,SOURCE,logs,lambda m,p:rpc(session,url,m,p),lambda rows:read(session,url,rows))


def snapshot(head,tag,factories,router,static,source,logs,fetch,views_at):
    rows=[]
    def code(addr):
        raw=bytes.fromhex(fetch("eth_getCode",[addr,tag])[2:]);return {"bytes":len(raw),"hash":"0x"+keccak(raw).hex()}
    bindings={}
    for name,addr in {**factories,"router":router,"routerStatic":static}.items():
        slot=fetch("eth_getStorageAt",[addr,SLOT,tag]);impl="0x"+slot[-40:]
        bindings[name]={"address":addr,**code(addr),"implementation":None if impl==ZERO else {"address":impl,**code(impl)}}
    for l in sorted(logs,key=lambda v:(int(v["blockNumber"],16),int(v["logIndex"],16))):
        sig,market,pt=l["topics"]
        if l.get("removed") or l["address"].lower() not in factories.values() or sig not in TOPICS:raise ValueError("неподтверждённая регистрация")
        rows.append({"factory":l["address"].lower(),"market":"0x"+market[-40:],"event_pt":"0x"+pt[-40:],"block":int(l["blockNumber"],16),"hash":l["blockHash"],
                    "tx":l["transactionHash"],"index":int(l["logIndex"],16),"event":sig,"args":l["data"]})
    if len({r["market"] for r in rows})!=len(rows):raise ValueError("повтор адреса модели")
    qs=[]
    for r in rows:
        m=r["market"]
        qs.extend([call(r["factory"],"isValidMarket(address)",["bool"],["address"],[m]),
                   call(m,"factory()",["address"]),call(m,"readTokens()",["address"]*3),call(m,"expiry()",["uint256"])])
    views=iter(views_at(qs));unique_sy=set()
    for r in rows:
        valid,factory,tokens,expiry=[next(views) for _ in range(4)]
        r.update(valid=valid==[True],bound_factory=factory[0] if factory else None,tokens=tokens,expiry=expiry[0] if expiry else None)
        if tokens:
            if tokens[1]!=r["event_pt"]:raise ValueError("регистрация PT расходится с рынком")
            unique_sy.add(tokens[0])
    sy_rows={};sy_list=sorted(unique_sy);qs=[]
    for a in sy_list:
        qs.extend([call(a,"getTokensIn()",["address[]"]),call(a,"getTokensOut()",["address[]"]),
                   call(a,"assetInfo()",["uint8","address","uint8"]),call(a,"exchangeRate()",["uint256"])])
    views=iter(views_at(qs))
    for a in sy_list:
        ins,outs,asset,rate=[next(views) for _ in range(4)]
        sy_rows[a]={"in":ins[0] if ins else None,"out":outs[0] if outs else None,"asset":asset,"rate":str(rate[0]) if rate else None}
    now=int(head["timestamp"],16);live=[r for r in rows if r["expiry"] and r["expiry"]>now and r["valid"] and r["tokens"]]
    qs=[]
    for r in live:
        sy,pt,yt=r["tokens"];m=r["market"]
        qs.extend([call(pt,"SY()",["address"]),call(pt,"YT()",["address"]),call(yt,"PT()",["address"]),call(yt,"SY()",["address"]),
                   call(pt,"expiry()",["uint256"]),call(yt,"expiry()",["uint256"]),
                   call(sy,"balanceOf(address)",["uint256"],["address"],[m]),call(pt,"balanceOf(address)",["uint256"],["address"],[m]),
                   call(m,"readState(address)",STATE,["address"],[router]),call(m,"readState(address)",STATE,["address"],[ZERO])])
    views=iter(views_at(qs))
    for r in live:
        sy,pt,yt=r["tokens"];ps,py,yp,ys,pe,ye,bs,bp,sr,sz=[next(views) for _ in range(10)]
        r["series"]=[ps,py,yp,ys,pe,ye]==[[sy],[yt],[pt],[sy],[r["expiry"]],[r["expiry"]]]
        r["physical"]=[str(bs[0]) if bs else None,str(bp[0]) if bp else None]
        r["router_state"]=list(map(str,sr)) if sr else None;r["plain_state"]=list(map(str,sz)) if sz else None
    return {"source":source,"selector":tag,"height":int(head["number"],16),"timestamp":now,"state_root":head["stateRoot"],"roles":bindings,
            "counts_by_factory":dict(Counter(r["factory"] for r in rows)),"registrations":rows,"standardized_yield":sy_rows}

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);args=ap.parse_args()
    failures=[]
    for endpoint in ENDPOINTS:
        try:
            d=capture(endpoint);args.output.write_text(json.dumps(d,separators=(",",":")),encoding="utf8");break
        except (requests.RequestException,ValueError,KeyError) as e:failures.append(str(e)[:240])
    else:raise RuntimeError(failures)
