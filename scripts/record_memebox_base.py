"""Memebox Base: registry, balances reserved for fees, and the maximum output paid by a real pair."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import OWNER, ROOT, call, calldata, rpc, simulate_block
from record_ethereum_v2_factories import registry

FACTORY="0x079463f811e6eb2e226908e79144cddb59a7fb71"
WETH="0x4200000000000000000000000000000000000006"
REFERENCE="0xa2b19417861c75ffc0ac68eb3e6a783f519130fe"


def tx(to,sig,types=(),args=(),value=0):
    return call(to,calldata(sig,types,args),value)


def uint(row):
    return int(row["returnData"],16)


def legs(c, market, funding, token_in, amount):
    pool=market["pool"];i=market["tokens"].index(token_in);token_out=market["tokens"][1-i]
    send=tx(token_in,"transfer(address,uint256)",["address","uint256"],[pool,amount])
    def batch(calls):
        return simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(100*10**18)}},calls)
    def balance(token,owner):
        return tx(token,"balanceOf(address)",["address"],[owner])
    state=[tx(pool,"getReserves()"),tx(pool,"balances()"),balance(token_in,pool),balance(token_out,pool),
           tx(pool,"feesTotal()"),balance(token_in,OWNER),balance(token_out,OWNER)]
    def data(rows):
        return dict(reserves=list(decode(["uint112","uint112","uint32"],bytes.fromhex(rows[0]["returnData"][2:])))[:2],
                    balances=list(decode(["uint256","uint256"],bytes.fromhex(rows[1]["returnData"][2:]))),
                    token_in=uint(rows[2]),token_out=uint(rows[3]),fees=list(decode(["uint256","uint256"],bytes.fromhex(rows[4]["returnData"][2:]))),
                    payer_in=uint(rows[5]),payer_out=uint(rows[6]))
    prefix=funding+state+[send]+state
    def exact(q):
        amounts=[0,0];amounts[1-i]=q
        return tx(pool,"swap(uint256,uint256,address,bytes)",["uint256","uint256","address","bytes"],[*amounts,OWNER,b""])
    start=batch(prefix)
    if not all(r["status"]=="0x1" for r in start):raise ValueError("wallet funding, transfer or a state query reverted")
    before=data(start[len(funding):len(funding)+len(state)]);funded=data(start[-len(state):])
    rin,rout=before["reserves"][i],before["reserves"][1-i];paid=funded["token_in"]-before["token_in"]
    def accepted(q):
        row=batch(prefix+[exact(q)])[-1]
        return row["status"]=="0x1",row.get("error")
    # A fixed search on the actual EVM, with speculative candidate values only to shorten the bracket.
    lo=0;hi=rout
    guesses=set()
    for den in (1000,10000,100000,1000000):
        fee=market["fee"]
        net=paid*(den-fee)
        if net<=0:continue
        for value in (net*rout//(rin*den+net), (paid-paid*fee//den)*rout//(rin+paid-paid*fee//den)):
            if 0<value<rout:guesses.update((value,value+1))
    # Reverts preceding the first success leave the simulated pair untouched.
    ordered=sorted((g for g in guesses if g<rout),reverse=True)
    trials=batch(prefix+[exact(q) for q in ordered])[-len(ordered):] if ordered else []
    for q,row in zip(ordered,trials):
        if row["status"]=="0x1":lo=q;break
        hi=q
    while hi-lo>1:
        mid=(lo+hi)//2;ok,_=accepted(mid)
        if ok:lo=mid
        else:hi=mid
    if lo==0:raise ValueError("no positive gross output passed")
    calls=batch(prefix+[exact(hi),exact(lo)]+state);j=len(prefix)
    if calls[j]["status"]!="0x0" or calls[j+1]["status"]!="0x1" or not all(r["status"]=="0x1" for r in calls[j+2:]):
        raise ValueError("final boundary did not replay")
    end=data(calls[j+2:]);received=end["payer_out"]-before["payer_out"]
    if received<=0 or before["payer_in"]-end["payer_in"]!=amount:raise ValueError("unconfirmed wallet deltas")
    return dict(input=str(amount),input_token=token_in,pair_input=str(paid),gross=str(lo),net=str(received),next=calls[j]["error"],
                before=before,funded=funded,after=end,gas=int(calls[j+1]["gasUsed"],16)),funding+[send,exact(lo)]


def capture(url):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes());c=Capture(dict(snap,endpoint=url))
    if int(rpc(url,"eth_chainId",[]),16)!=8453:raise ValueError("wrong chain")
    init,=c.one(FACTORY,"INIT_CODE_HASH()",returns=["bytes32"])
    rows=registry(c,FACTORY,"0x"+init.hex())
    fields=iter(posts(c,[op for m in rows for op in [(m["pool"],"poolFee()",[],[]),(m["pool"],"protocolRatio()",[],[]),
              (m["pool"],"feesTotal()",[],[])]+[(t,"balanceOf(address)",["address"],[m["pool"]]) for t in m["tokens"]]]))
    for m in rows:
        m["fee"]=decode(["uint16"],next(fields))[0];m["protocol_ratio"]=decode(["uint256"],next(fields))[0]
        m["fees_total"]=list(map(str,decode(["uint256","uint256"],next(fields))))
        m["balances"]=[str(decode(["uint256"],next(fields))[0]) for _ in m["tokens"]]
    cases=[]
    for market in [next(m for m in rows if m["pool"]==REFERENCE),max((m for m in rows if WETH in m["tokens"]),key=lambda m:int(m["reserves"][m["tokens"].index(WETH)]))]:
        i=market["tokens"].index(WETH);rin=int(market["reserves"][i])
        for size in (min(10**15,rin//1000),min(10**16,rin//25)):
            buy,fund=legs(c,market,[tx(WETH,"deposit()",value=size)],WETH,size)
            sell,_=legs(c,market,fund,market["tokens"][1-i],int(buy["net"]))
            cases.append(dict(pool=market["pool"],buy=buy,sell=sell))
    return dict(factory=FACTORY,creation_hash="0x"+init.hex(),selector=c.selector,chain=8453,markets=rows,cases=cases)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);a=ap.parse_args()
    a.output.write_text(json.dumps(capture("https://mainnet.base.org"),indent=2)+"\n",encoding="utf8")
