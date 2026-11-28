"""Raindex: фактический расход и выплата независимому кошельку в изолированном дочернем блоке.
На покупку используется только обёрнутый собственный ETH либо выданный ему Uniswap стейбл.
Имеющиеся у maker vault и внешние оракулы не подменяются. Подписи maker не изготавливаются.
"""
import argparse
from decimal import Decimal, localcontext
import gzip
import json
from pathlib import Path
import requests
from eth_abi import encode
from eth_utils import keccak
from record_base_slipstream_executor import OWNER,call,rpc
from record_raindex_quotes import DATA,data
from raindex_wire import take,decimal,SELECTORS

ROOT=Path(__file__).resolve().parents[1]
WETH={"base":"0x4200000000000000000000000000000000000006","ethereum":"0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"}
USDC={"base":"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913","ethereum":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"}
USDT="0xdac17f958d2ee523a2206206994597c13d831ec7"
GOOD=lambda op: op["status"]=="0x1"
LIMIT="(address,address,uint256,uint256,uint256,uint256)"
def tx(target,name,types,values,value=0):
    return call(target,data(name,types,values),value,gas=10_000_000)


def capture(network,url,artifact,legacy=False,*,selected_books=None):
    src=json.loads(gzip.decompress((DATA/f"raindex_{network}_quotes.json.gz").read_bytes()))
    tag=src["anchor"];is_base=network=="base";chain=8453 if is_base else 1
    head=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    if head["hash"]!=tag["blockHash"] or int(rpc(url,"eth_chainId",[]),16)!=chain:raise ValueError("chain")
    stamp=int(head["timestamp"],16)+(2 if is_base else 12); end=stamp+300
    if rpc(url,"eth_getCode",[OWNER,tag])!="0x":raise ValueError("occupied payer")
    overrides={OWNER:{"balance":hex(4*10**18)}};identities={}
    def run(calls):
        [child]=rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"blockOverrides":{"time":hex(stamp)},"stateOverrides":overrides,"calls":calls}]},tag])
        if child["parentHash"]!=tag["blockHash"] or len(child["calls"])!=len(calls):raise ValueError("child")
        return child["calls"]
    books=([{"book":src["book"],"version":6,"runtime":src["runtime"],"live":src["entries"]}] if is_base else src["books"])
    if selected_books is not None:
        books=selected_books
    if legacy:books=[b for b in books if b["version"]==3]
    def module(b):
        book=b["book"]
        if book in identities:return identities[book]["address"]
        addr="0x"+keccak(text="Raindex protected isolated wallet "+book)[-20:].hex()
        if rpc(url,"eth_getCode",[addr,tag])!="0x":raise ValueError("occupied module")
        args=encode(["address","bytes32","bytes4"],[book,bytes.fromhex(b["runtime"][2:]),bytes.fromhex(SELECTORS[b["version"]])])
        creation=artifact["creation"].removeprefix("0x")+args.hex()
        # Получаем точный runtime с constructor immutables при исходном коде настоящей книги.
        r=requests.post(url,json={"id":1,"jsonrpc":"2.0","method":"eth_call","params":[{"from":OWNER,"data":"0x"+creation},tag]},timeout=18)
        r.raise_for_status();raw=bytes.fromhex(r.json()["result"][2:]);blank=bytes.fromhex(artifact["runtime"].removeprefix("0x"));masked=bytearray(raw);words=set()
        for refs in artifact["immutableReferences"].values():
            values=set()
            for field in refs:
                k,n=field["start"],field["length"]
                values.add(raw[k:k+n]);masked[k:k+n]=blank[k:k+n]
            if len(values)!=1:raise ValueError("immutable copies")
            words.update(values)
        if masked!=blank or words!={args[i:i+32] for i in range(0,len(args),32)}:raise ValueError("book binding")
        overrides[addr]={"code":"0x"+raw.hex()};identities[book]={"address":addr,"runtime":"0x"+keccak(raw).hex(),"book":book,"version":b["version"],"selector":SELECTORS[b["version"]]}
        return addr
    weth=WETH[network];usd=USDC[network];stable=usd if is_base else USDT
    router=(next(x["settings"]["dex.router"] for x in json.loads((ROOT/"config/first_run_dex.json").read_bytes()) if x["id"]=="uniswap-v2-base") if is_base else "0x7a250d5630b4cf539739df2c5dacb4c659f2488d")
    seed=[tx(weth,"deposit",[],[],10**17),tx(router,"swapExactETHForTokens",["uint256","address[]","address","uint256"],[1,[weth,stable],OWNER,end],10**17)]
    if legacy:
        seed=[tx(router,"swapExactETHForTokens",["uint256","address[]","address","uint256"],[1,[weth,stable],OWNER,end],2*10**18)]
    if not all(map(GOOD,run(seed))):raise ValueError("own funding")
    def bal(token,owner=OWNER):return tx(token,"balanceOf",["address"],[owner])
    def approve(token,to,v):return tx(token,"approve",["address","uint256"],[to,v])
    def units(row):return int(row["returnData"],16)
    def money(prefix,b,o,l,cap,size,receive_fixed,di,do,partial=False):
        source,target=l["pay"],l["receive"];book=b["book"];version=b["version"];where=module(b) if artifact else book
        action=take(version,o["order"],l["i"],l["j"],1 if partial else size,size,do if receive_fixed else di,receive_fixed)
        setup=prefix+[approve(source,where,cap)]
        checks=[bal(source),bal(target)];held=[]
        if artifact:held=[bal(source,where),bal(target,where),tx(source,"allowance",["address","address"],[where,book])]
        def play(low,high,floor,deadline):
            payment=(tx(where,"fill",[LIMIT,"bytes"],[(source,target,low,high,floor,deadline),bytes.fromhex(action[2:])]) if artifact else call(book,action,gas=10_000_000))
            rows=run(setup+checks+held+[payment]+checks+held)
            ix=len(setup)+len(checks)+len(held);op=rows[ix]
            if not all(map(GOOD,rows[:ix]+rows[ix+1:])):raise ValueError("funding or balance")
            before=[units(x) for x in rows[len(setup):ix]]
            after=[units(x) for x in rows[ix+1:]]
            return payment,op,before,after
        payment,op,be,af=play(1,cap,1,end)
        base={"book":book,"version":version,"order":o["hash"],"i":l["i"],"j":l["j"],"pay":source,"receive":target,
              "fixed_receive":receive_fixed,"partial":partial,"size":str(size),"cap":str(cap)}
        if not GOOD(op):return None,dict(base,stage="revert",error=op.get("error"))
        paid=be[0]-af[0];net=af[1]-be[1]
        if net<=0 or paid<=0 or paid>cap or be[2:]!=af[2:]:return None,dict(base,stage="incomplete",before=be,after=af)
        if artifact:
            payment,op,b2,a2=play(paid,paid,net,end)
            if not GOOD(op) or (be,af)!=(b2,a2):raise ValueError("protected tight bound")
            for low,high,floor,dl in [(paid,paid,net+1,end),(1,paid-1,net,end),(paid,paid,net,stamp-1)]:
                _,bad,x,y=play(low,high,floor,dl)
                if GOOD(bad) or x!=y:raise ValueError("protected bound rollback")
        return setup+[payment],dict(base,stage="paid",paid=str(paid),net=str(net),before=[str(v) for v in be],after=[str(v) for v in af],
                                   action=action,target=where,gas=units({"returnData":op["gasUsed"]}),logs=op["logs"])
    legs=[]
    for b in books:
        for o in b["live"]:
            for l in o.get("quotes",o.get("legs",[])):
                if b["version"]!=3 and (not l.get("live") or not int(l["ratio"],16) or not int(l["cap"],16)):continue
                legs.append((b,o,l))
    pairs=[]
    decimals={}
    def dec(token):
        if token not in decimals:
            [r]=run([tx(token,"decimals",[],[])]);decimals[token]=units(r)
        return decimals[token]
    for b,o,l in legs:
        pay,out=l["pay"],l["receive"]
        if pay not in (stable,weth):continue

        record={"pay":pay,"receive":out,"cases":[]};pairs.append(record)
        for scale in ((80,84,85,100,200) if legacy else (1,100,500)):
            df,dt=dec(pay),dec(out);both=b["version"]>=6
            # Older books receive the requested asset; signed maximum capital still comes from this payer.
            budget=scale*(10_000 if df==6 else 10**15)
            fixed_size=max(1,scale*10**dt//100)
            if legacy:
                budget=3_000*10**df;fixed_size=scale*10**dt
            if both or (selected_books is not None and b["version"]==5):
                ratio=decimal(l["ratio"])
                possible=int(Decimal(budget).scaleb(-df)/ratio*Decimal(10**dt)) if ratio>0 else 0
                fixed_size=max(1,min(possible*9//10,int(decimal(l["cap"])*Decimal(10**dt))))
            case={"scale":scale};record["cases"].append(case)
            for buy_output in ([False,True] if both else [True]):
                size=fixed_size if buy_output else budget
                funded,buy=money(seed,b,o,l,budget,size,buy_output,df,dt);key="by_output" if buy_output else "by_debit";case[key]=buy
                if not funded and artifact and b["version"]==4 and buy_output:
                    # The module pulls the whole signed cap. A broad price budget can exceed the wallet,
                    # even though the canonical cost of the requested output is affordable.
                    n=size*int(l["ratio"],16)*10**df
                    d=10**(18+dt)
                    quoted=(n+d-1)//d
                    if 0<quoted<budget:
                        case[key+"_wide_cap"]=buy
                        funded,buy=money(seed,b,o,l,quoted,size,True,df,dt)
                        case[key]=buy
                if not funded:continue
                reverses=[v for v in legs if v[2]["pay"]==out and v[2]["receive"]==pay]
                for rb,ro,rl in reverses:
                    # Only the purchased quantity is available for this reverse order.
                    cap=int(buy["net"]);mode=rb["version"]<6
                    want=max(1,int(buy["paid"])//10) if mode else cap
                    _,sell=money(funded,rb,ro,rl,cap,want,mode,dt,df,partial=True)
                    if sell["stage"]=="paid":
                        case[key+"_reverse"]=sell;break
                    case.setdefault(key+"_reverse_attempts",[]).append({f:sell[f] for f in ("order","stage","error") if f in sell})
        if both and any(v.get("stage")=="paid" for c in record["cases"] for v in c.values() if isinstance(v,dict)):
            # Больше текущего outputMax одного maker. Оплачиваем только достигнутую часть и возвращаем остаток своего cap.
            with localcontext() as precision:
                precision.prec=100
                capacity=int(decimal(l["cap"])*Decimal(10**dt))
                cost=int((decimal(l["cap"])*decimal(l["ratio"])*Decimal(10**df)).to_integral_value(rounding="ROUND_CEILING"))
            bound=cost+max(2,cost//10)
            maximum=capacity+max(2,capacity//2)
            _,limited=money(seed,b,o,l,bound,maximum,True,df,dt,partial=True)
            if limited["stage"]=="paid" and not (0<int(limited["net"])<maximum and int(limited["paid"])<bound):
                raise ValueError("expected a partial payment and change")
            record["partial_capacity"]=limited
    return {"chain":chain,"selector":tag,"height":int(head["number"],16),"stamp":stamp,"payer":OWNER,"seed":seed,"modules":identities,"pairs":pairs}


if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("network",choices=["base","ethereum"]);p.add_argument("out",type=Path);p.add_argument("--module",type=Path);p.add_argument("--legacy",action="store_true");args=p.parse_args()
    urls=(["https://base.drpc.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"] if args.network=="base" else ["https://eth.drpc.org","https://mainnet.gateway.tenderly.co","https://ethereum-rpc.publicnode.com"])
    errors=[]
    for url in urls:
        try:
            archive=capture(args.network,url,json.loads(args.module.read_bytes()) if args.module else None,args.legacy)
            args.out.write_text(json.dumps(archive,separators=(",",":")),encoding="utf8");break
        except (requests.RequestException,ValueError,KeyError) as error:errors.append(str(error)[:200])
    else:raise ValueError(errors)
