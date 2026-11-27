"""Ethereum: фактический расход USDT при покупке WETH и оплате активного Raindex V4.
Каждая проверка использует собственное финансирование, канонический родитель и реальные контракты.
"""
import argparse
import gzip
import json
from pathlib import Path
import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_raindex_wallets import DATA, GOOD, OWNER, USDT, WETH, call, data, rpc, tx

ROUTER="0x7a250d5630b4cf539739df2c5dacb4c659f2488d"
REQUEST="(address,uint256,uint256,uint256,uint256,bool)"


def run_archive(url, folder):
    originals=json.loads(gzip.decompress((DATA/"raindex_ethereum_protected_wallets.json.gz").read_bytes()))
    tag=originals["selector"]
    h=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    if int(rpc(url,"eth_chainId",[]),16)!=1 or h["hash"]!=tag["blockHash"]:
        raise ValueError("chain")
    stamp=int(h["timestamp"],16)+12
    end=stamp+300
    weth=WETH["ethereum"]
    overrides={OWNER:{"balance":hex(10**18)}}

    def measured(calls):
        [child]=rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"blockOverrides":{"time":hex(stamp)},"stateOverrides":overrides,"calls":calls}]},tag])
        if child["parentHash"]!=tag["blockHash"] or len(child["calls"])!=len(calls):raise ValueError("child")
        return child["calls"]

    def canonical(addr):
        code=rpc(url,"eth_getCode",[addr,tag]);return "0x"+keccak(hexstr=code).hex()

    def address(addr,name,types=(),values=()):
        [ret]=measured([tx(addr,name,list(types),list(values))])
        if not GOOD(ret):raise ValueError(name)
        return decode(["address"],bytes.fromhex(ret["returnData"][2:]))[0].lower()

    def module(name,types,values):
        artifact=json.loads((folder/(name+".json")).read_bytes())
        addr="0x"+keccak(text="Raindex cash experiment "+name)[-20:].hex()
        if rpc(url,"eth_getCode",[addr,tag])!="0x":raise ValueError("occupied experimental address")
        creation=artifact["creation"].removeprefix("0x")+encode(types,values).hex()
        reply=requests.post(url,json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":[{"from":OWNER,"data":"0x"+creation},tag,overrides]},timeout=25)
        reply.raise_for_status();raw=bytes.fromhex(reply.json()["result"][2:])
        blank=bytes.fromhex(artifact["runtime"].removeprefix("0x"));masked=bytearray(raw)
        for refs in artifact["immutableReferences"].values():
            copies=set()
            for ref in refs:
                k,n=ref["start"],ref["length"];copies.add(raw[k:k+n]);masked[k:k+n]=blank[k:k+n]
            if len(copies)!=1:raise ValueError("immutable copies")
        if masked!=blank:raise ValueError("runtime mismatch")
        overrides[addr]={"code":"0x"+raw.hex()}
        return addr,"0x"+keccak(raw).hex()

    books=json.loads(gzip.decompress((DATA/"raindex_ethereum_quotes.json.gz").read_bytes()))["books"]
    v4=next(b for b in books if b["version"]==4)
    original=originals["modules"][v4["book"]]
    wallet,inner_hash=module("RaindexWallet",["address","bytes32","bytes4"],[v4["book"],bytes.fromhex(v4["runtime"][2:]),bytes.fromhex(original["selector"])])
    if inner_hash!=original["runtime"]:raise ValueError("book wallet differs")
    factory=address(ROUTER,"factory")
    pair=address(factory,"getPair",["address","address"],[USDT,weth])
    wrapper,runtime=module("RaindexCashWallet",["address"]*5,[wallet,ROUTER,pair,USDT,weth])
    for name,expected in [("bookWallet",wallet),("router",ROUTER),("pair",pair),("cash",USDT),("bridge",weth)]:
        if address(wrapper,name)!=expected:raise ValueError("wrong role "+name)
    seed=tx(ROUTER,"swapExactETHForTokens",["uint256","address[]","address","uint256"],[1,[weth,USDT],OWNER,end],10**17)
    def bal(asset,who):return tx(asset,"balanceOf",["address"],[who])
    def allowance(asset,owner,spender):return tx(asset,"allowance",["address","address"],[owner,spender])
    def approve(cap):return tx(USDT,"approve",["address","uint256"],[wrapper,cap])
    def uint(op):return int(op["returnData"],16)
    def run(asset,action,cap,minimum,mid,deadline):
        checks=[bal(USDT,OWNER),bal(asset,OWNER),bal(weth,OWNER),
                bal(USDT,wrapper),bal(asset,wrapper),bal(weth,wrapper),bal(weth,wallet),bal(asset,wallet),
                allowance(weth,wrapper,wallet),allowance(USDT,wrapper,ROUTER),allowance(weth,wrapper,ROUTER)]
        order=tx(wrapper,"fill",[REQUEST,"bytes"],[(asset,cap,minimum,mid,deadline,True),bytes.fromhex(action[2:])])
        rows=measured([seed,approve(cap),*checks,order,*checks]); pos=2+len(checks)
        if not all(GOOD(s) for i,s in enumerate(rows) if i!=pos):raise ValueError("funding/checks")
        return rows[pos],list(map(uint,rows[2:pos])),list(map(uint,rows[pos+1:]))
    cases=[]
    for market in originals["pairs"]:
        for case in market["cases"]:
            row=case["by_output"]
            if row["stage"]!="paid" or row["version"]!=4 or row["pay"]!=weth:continue
            asset=row["receive"];target=int(row["net"]);required=int(row["paid"])
            for acquired in (required,required+required//10):
                quote=tx(ROUTER,"getAmountsIn",["uint256","address[]"],[acquired,[USDT,weth]])
                a,b=measured([seed,quote]);assert GOOD(a) and GOOD(b)
                amounts=decode(["uint256[]"],bytes.fromhex(b["returnData"][2:]))[0]
                cap=amounts[0]+max(2,amounts[0]//20)
                op,pre,post=run(asset,row["action"],cap,target,acquired,end)
                if not GOOD(op):raise ValueError(("settlement",op.get("error"),cap,target,acquired))
                spent=pre[0]-post[0];credit=post[1]-pre[1]
                if not (0<spent<=cap and credit==target and pre[2:]==post[2:]):raise ValueError("wallet delta")
                for limit,floor,deadline in [(spent-1,target,end),(cap,target+1,end),(cap,target,stamp-1)]:
                    bad,x,y=run(asset,row["action"],limit,floor,acquired,deadline)
                    if GOOD(bad) or x!=y:raise ValueError("boundary")
                cases.append(dict(order=row["order"],asset=asset,payer=OWNER,input=USDT,bridge=weth,action=row["action"],cap=str(cap),minimum=str(target),
                                  acquired=str(acquired),bookPaid=str(required),spent=str(spent),received=str(credit),gas=int(op["gasUsed"],16),
                                  before=list(map(str,pre)),after=list(map(str,post)),logs=op["logs"]))
    if len(cases)<4:raise ValueError("no positive size matrix")
    return dict(chain=1,selector=tag,height=int(h["number"],16),stamp=stamp,payer=OWNER,router=ROUTER,routerCode=canonical(ROUTER),pair=pair,pairCode=canonical(pair),
                factory=factory,wallet=wallet,walletCode=inner_hash,wrapper=wrapper,wrapperCode=runtime,cash=USDT,bridge=weth,cases=cases)


if __name__=="__main__":
    arg=argparse.ArgumentParser();arg.add_argument("folder",type=Path);arg.add_argument("out",type=Path);a=arg.parse_args()
    errs=[]
    for url in ("https://eth.drpc.org","https://mainnet.gateway.tenderly.co","https://ethereum-rpc.publicnode.com"):
        try:
            a.out.write_text(json.dumps(run_archive(url,a.folder),separators=(",",":")),encoding="utf8");break
        except (ValueError,requests.RequestException,KeyError) as e:errs.append(str(e)[:180])
    else:raise ValueError(errs)
