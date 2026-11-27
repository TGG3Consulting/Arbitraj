"""Compare funded direct exact-input/output and existing ArbExecutor V5 step 9 on Ethereum LLAMMAs."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import tx, token_flow
from record_llamma_ethereum import EVENT, TRANSFER, UINT
from record_mooniswap import CanonicalCalls, FUNDING, Reader, WETH, read_balance
from record_yield_basis_usdt import BRIDGE, USDT

STABLE = "0xf939e0a03fb07f59a73314e73794be0e57ac1b4e"
SAMPLE = {"mint":[3], "oneway":[1,3,9], "v2":[1]}
ARGS=[UINT]*4


def approve(token,who,qty):
    return tx(token,"approve(address,uint256)",["address",UINT],[who,qty])


def base_funding():
    # Buy liquid USDT with the simulated payer's ETH and actually exchange it for crvUSD.
    amount=100*10**6
    return [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",[UINT,"address[]","address",UINT],
               [amount,[WETH,USDT],OWNER,2**64-1],value=10**18),
            approve(USDT,BRIDGE,amount),tx(BRIDGE,"exchange(int128,int128,uint256,uint256)",["int128","int128",UINT,UINT],[0,1,amount,1])]


def trial(reader, env, m, prefix, side, cap, mode, deadline):
    amm=m["amm"];tokens=[m["borrowed"],m["collateral"]];src,dst=tokens[side],tokens[1-side]
    target=amm if mode!="v5" else EXEC
    def sim(cmds):
        return simulate_block(None,reader.tag,env,cmds,read_rpc=lambda _,method,params:reader.one(method,params))
    def order(number,limit):
        if mode=="v5":
            step=(9,amm,src,dst,10000,encode([UINT,UINT],[side,1-side]))
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address",UINT,"address",UINT,UINT,STEP+"[]"],
                      [src,number,dst,limit,deadline,[step]])
        name="exchange_dy" if mode=="target" else "exchange"
        return tx(amm,name+"(uint256,uint256,uint256,uint256,address)",ARGS+["address"],[side,1-side,number,limit,OWNER])
    quotes=[tx(amm,"get_dxdy(uint256,uint256,uint256)",[UINT]*3,[side,1-side,cap])]
    paid_prefix=prefix+[approve(src,target,cap)]
    q=sim(paid_prefix+quotes)
    if any(row["status"]!="0x1" for row in q[:-1]):return dict(status="funding_failed",reason=next(r.get("error") for r in q[:-1] if r["status"]!="0x1"))
    if q[-1]["status"]!="0x1":return dict(status="quote_reverted",reason=q[-1].get("error"))
    use,out=decode([UINT,UINT],bytes.fromhex(q[-1]["returnData"][2:]))
    if not use or not out:return dict(status="no_liquidity",spent=str(use),received=str(out))
    if mode=="target":
        exact=sim(paid_prefix+[tx(amm,"get_dydx(uint256,uint256,uint256)",[UINT]*3,[side,1-side,out])])[-1]
        if exact["status"]!="0x1":return dict(status="target_quote_reverted",reason=exact.get("error"))
        got,use=decode([UINT,UINT],bytes.fromhex(exact["returnData"][2:]))
        if got!=out or use>cap:return dict(status="target_outside_cap",spent=str(use),received=str(got))
    bound=order(out,use) if mode=="target" else order(cap,out)
    negative=order(out,use-1) if mode=="target" else order(cap,out+1)
    marks=[read_balance(t,a) for a in [OWNER,amm,EXEC] for t in [src,dst]]
    rows=sim(paid_prefix+marks+[negative,bound]+marks)
    pos=len(paid_prefix);high,trade=rows[pos+len(marks):pos+len(marks)+2]
    if high["status"]!="0x0":raise ValueError(("bound passed",m["index"],side,mode))
    if trade["status"]!="0x1":return dict(status="trade_reverted",reason=trade.get("error"),preview=[str(use),str(out)])
    if any(row["status"]!="0x1" for row in rows[:pos+len(marks)]+rows[-len(marks):]):raise ValueError("bad funding or balance view")
    before=[int(v["returnData"],16) for v in rows[pos:pos+len(marks)]]
    after=[int(v["returnData"],16) for v in rows[-len(marks):]]
    delta=[b-a for a,b in zip(before,after,strict=True)]
    if delta!=[-use,out,use,-out,0,0]:raise ValueError(("wallet/pool/executor",m["index"],mode,delta,use,out))
    events=[l for l in trade["logs"] if l["address"].lower()==amm and l["topics"][0]==EVENT]
    if len(events)!=1 or decode([UINT]*4,bytes.fromhex(events[0]["data"][2:]))!=(side,use,1-side,out):
        raise ValueError("different physical fill")
    if ("0x"+events[0]["topics"][1][-40:].lower())!=(EXEC if mode=="v5" else OWNER):
        raise ValueError("foreign output receiver")
    executor_refund=sum(int(l["data"],16) for l in trade["logs"] if l["address"].lower()==src and len(l["topics"])==3
                        and l["topics"][0]==TRANSFER and "0x"+l["topics"][1][-40:].lower()==EXEC
                        and "0x"+l["topics"][2][-40:].lower()==OWNER)
    if executor_refund!=(cap-use if mode=="v5" else 0):
        raise ValueError(("unconfirmed change",executor_refund,mode,cap-use))
    return dict(status="paid",cap=str(cap),spent=str(use),received=str(out),unusedCap=str(cap-use),
                returnedByExecutor=str(executor_refund),gas=int(trade["gasUsed"],16),
                approvalGas=int(rows[pos-1]["gasUsed"],16),before=list(map(str,before)),after=list(map(str,after)),
                control=negative,controlError=high["error"],trade=bound,logs=trade["logs"])


def usdt_path(reader,env,m):
    amm=m["amm"]
    if (m["borrowed"],m["collateral"])!=(STABLE,WETH):
        raise ValueError("unexpected crvUSD/WETH exchange")
    assets=[USDT,STABLE,WETH]
    marks=[read_balance(t,who) for who in (OWNER,EXEC) for t in assets]
    end=int(reader.header["timestamp"],16)+300
    rows=[]
    for sell in (False,True):
        src,dst=(WETH,USDT) if sell else (USDT,WETH)
        cur=(4,BRIDGE,STABLE if sell else USDT,USDT if sell else STABLE,10000,encode(["int128"]*2,[int(sell),int(not sell)]))
        lev=(9,amm,WETH if sell else STABLE,STABLE if sell else WETH,10000,encode([UINT]*2,[int(sell),int(not sell)]))
        steps=[lev,cur] if sell else [cur,lev]
        for amount in ((10**15,10**16) if sell else (10**6,10**7)):
            acquire=[tx(WETH,"deposit()",value=amount)] if sell else [
                tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",[UINT,"address[]","address",UINT],
                   [amount,[WETH,USDT],OWNER,end],value=10**17)]
            prep=acquire+[approve(src,EXEC,amount)]
            def order(minimum,deadline):
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address",UINT,"address",UINT,UINT,STEP+"[]"],
                          [src,amount,dst,minimum,deadline,steps])
            def sim(calls):
                all_rows=simulate_block(None,reader.tag,env,prep+marks+calls+marks,read_rpc=lambda _,method,params:reader.one(method,params))
                n=len(prep);a=all_rows[n:n+len(marks)];b=all_rows[-len(marks):]
                if any(v["status"]!="0x1" for v in all_rows[:n]+a+b):raise ValueError("route funding or balance")
                return all_rows[n+len(marks): -len(marks)],[int(y["returnData"],16)-int(x["returnData"],16) for x,y in zip(a,b,strict=True)],int(all_rows[n-1]["gasUsed"],16)
            first,delta,ap=sim([order(1,end)])
            if first[0]["status"]!="0x1":
                rows.append(dict(source=src,target=dst,cap=str(amount),status="refused",error=first[0].get("error")));continue
            net=delta[assets.index(dst)];spent=-delta[assets.index(src)]
            if not 0<spent<=amount or net<=0 or delta[1]<0 or any(delta[3:]):raise ValueError(("composition accounting",delta))
            receipt,exact,_=sim([order(net+1,end),order(net,0),order(net,end)])
            if [o["status"] for o in receipt]!=["0x0","0x0","0x1"] or exact!=delta:raise ValueError("composition controls")
            ll=[l for l in receipt[2]["logs"] if l["address"].lower()==amm and l["topics"][0]==EVENT]
            if len(ll)!=1:raise ValueError("unrelated LLAMMA route")
            i,x,j,y=decode([UINT]*4,bytes.fromhex(ll[0]["data"][2:]))
            if (i,j)!=(int(sell),int(not sell)):raise ValueError("reverse physical LLAMMA path")
            bridge_stable=token_flow(receipt[2]["logs"],STABLE,BRIDGE)
            bridge_usdt=token_flow(receipt[2]["logs"],USDT,BRIDGE)
            mid_in,mid_out=(y,bridge_stable) if sell else (-bridge_stable,x)
            if not mid_out>0 or mid_in-mid_out!=delta[1] or bridge_usdt!=(-net if sell else spent):
                raise ValueError(("bridge cash mismatch",bridge_stable,bridge_usdt,delta))
            rows.append(dict(source=src,target=dst,status="paid",cap=str(amount),spent=str(spent),received=str(net),
                             intermediateReceived=str(mid_in),intermediateSpent=str(mid_out),
                             changeCrvUSD=str(delta[1]),gas=int(receipt[2]["gasUsed"],16),approvalGas=ap,steps=[s[0] for s in steps],
                             trade=order(net,end),logs=receipt[2]["logs"],control=[o["status"] for o in receipt]))
    if {r["source"] for r in rows if r["status"]=="paid"}!={USDT,WETH}:raise ValueError("missing bidirectional terminal cash")
    return rows


def capture():
    directory=ROOT/"tests/data/first_run_dex_2026-11-26"
    book=json.loads(gzip.decompress((directory/"llamma_ethereum_registry.json.gz").read_bytes()))
    reader=Reader("ethereum",json.loads((directory/"ethereum.json").read_bytes()));cap=CanonicalCalls(reader)
    if reader.tag!=book["selector"] or reader.header["stateRoot"]!=book["root"]:raise ValueError("different registry head")
    bridge=[cap.one(BRIDGE,"coins(uint256)",[UINT],[i],["address"])[0] for i in (0,1)]
    if bridge!=[USDT,STABLE]:raise ValueError("funding bridge currencies")
    runtime="0x"+RUNTIME.read_text().strip().removeprefix("0x");word=lambda a:"0x"+format(int(a,16),"064x")
    env={OWNER:{"balance":hex(100*10**18)},EXEC:{"code":runtime,"stateDiff":{word("0"):word(OWNER),word("1"):word(WETH)}}}
    if any(reader.one("eth_getCode",[a,reader.tag])!="0x" for a in env):raise ValueError("occupied isolated identities")
    rows=[]
    for kind,indexes in SAMPLE.items():
        for index in indexes:
            m=book["markets"][kind][index]
            if m["index"]!=index or m["borrowed"]!=STABLE:raise ValueError("wrong funding currency")
            for budget in (10**18,10**19):
                prefix=base_funding();ts=int(reader.header["timestamp"],16)+300
                buy=trial(reader,env,m,prefix,0,budget,"source",ts)
                for mode in ("source","target","v5"):
                    order=buy if mode=="source" else trial(reader,env,m,prefix,0,budget,mode,ts)
                    rows.append(dict(kind=kind,index=index,side=0,mode=mode,budget=str(budget),result=order))
                if buy["status"]!="paid":continue
                # The reverse leg starts after the same real paid purchase, using the exact newly acquired inventory.
                prefix += [approve(STABLE,m["amm"],budget),buy["trade"]]
                for mode in ("source","target","v5"):
                    reverse=trial(reader,env,m,prefix,1,int(buy["received"]),mode,ts)
                    rows.append(dict(kind=kind,index=index,side=1,mode=mode,budget=str(budget),postBuy=True,result=reverse))
    # A plain WETH deposit leaves the market in its original state. One-way market 1 has
    # less WETH-side depth than this cap; the owner must recover the unconsumed principal.
    m=book["markets"]["oneway"][1]
    if m["collateral"]!=WETH:raise ValueError("changed partial-input market")
    for amount in (10**15,10**16):
        prefix=[tx(WETH,"deposit()",value=amount)]
        for mode in ("source","target","v5"):
            partial=trial(reader,env,m,prefix,1,amount,mode,int(reader.header["timestamp"],16)+300)
            if partial["status"]!="paid" or not 0<int(partial["spent"])<amount:
                raise ValueError(("partial deposit evidence",mode,partial.get("status"),partial.get("spent")))
            rows.append(dict(kind="oneway",index=1,side=1,mode=mode,budget=str(amount),funding="WETH.deposit",
                             result=partial))
    if not all(any(r["kind"]==k and r["mode"]==mode and r["result"]["status"]=="paid" for r in rows)
               for k in SAMPLE for mode in ("source","target","v5")):
        raise ValueError("a selected factory or wire has no paid result")
    return dict(selector=reader.tag,owner=OWNER,executor=EXEC,bridge=BRIDGE,rows=rows,usdt=usdt_path(reader,env,book["markets"]["mint"][3]))


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);a=ap.parse_args()
    a.output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf-8")

