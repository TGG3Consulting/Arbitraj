"""Канонические значения бинов E3, физическая комиссия, остаток и выход на каждой стороне."""
import argparse
import json
from pathlib import Path
from record_eulerswap_directory import Reader
from record_e3_base import FACTORY,ROUTER

MAX_ID=(1<<24)-1
def capture():
    r=Reader("base")
    n,=r.many([(FACTORY,"getNumberOfLBPairs()",(),"uint256")])
    addresses=r.many([(FACTORY,"getLBPairAtIndex(uint256)",(i,),"address") for i in range(n)])
    pools=[]
    for p in addresses:
        active,step,s,v,x,y=r.many([(p,fn,(),ty) for fn,ty in [
            ("getActiveId()","uint24"),("getBinStep()","uint16"),
            ("getStaticFeeParameters()","uint16,uint16,uint16,uint16,uint24,uint16,uint24"),
            ("getVariableFeeParameters()","uint24,uint24,uint24,uint40"),("getTokenX()","address"),("getTokenY()","address")]])
        ids={active};ends=[]
        for direction in (False,True):
            current=active
            for _ in range(512):
                following,=r.many([(p,"getNextNonEmptyBin(bool,uint24)",(direction,current),"uint24")])
                if following in (0,MAX_ID):
                    ends.append(following);break
                assert following not in ids and (following<current)==direction
                ids.add(following);current=following
            else:raise ValueError("Не достигнут последний бин "+p)
        bids=sorted(ids)
        balances=r.many([(p,"getBin(uint24)",(i,),"uint128,uint128") for i in bids])
        decimals=r.many([(z,"decimals()",(),"uint8") for z in (x,y)])
        qs=[]
        for index,direction in enumerate([True,False]):
            max_input=10**28
            left,cap,fee=r.many([(p,"getSwapOut(uint128,bool)",(max_input,direction),"uint128,uint128,uint128")])[0]
            ceiling=max_input-left
            sizes={1,max(1,10**decimals[index]//10**6),max(1,10**decimals[index]//1000),10**decimals[index],max_input}
            sizes.update(z for z in (ceiling-1,ceiling,ceiling+1,ceiling//2) if z>0)
            qty=sorted(sizes)
            quotes=r.many([(p,"getSwapOut(uint128,bool)",(a,direction),"uint128,uint128,uint128") for a in qty])
            assert all(z is not None for z in quotes)
            qs.extend([dict(for_y=direction,qty=str(a),result=list(map(str,z))) for a,z in zip(qty,quotes)])
        pools.append(dict(pool=p,x=x,y=y,active=active,step=step,statics=s,variable=v,bins=[[i,str(b[0]),str(b[1])] for i,b in zip(bids,balances)],ends=ends,quotes=qs))
    head=r.rpc("eth_getBlockByHash",[r.tag["blockHash"],False])
    return dict(selector=r.tag,stamp=int(head["timestamp"],16),factory=FACTORY,router=ROUTER,pools=pools)
if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output",type=Path)
    ap.parse_args().output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

