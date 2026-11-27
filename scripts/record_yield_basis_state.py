"""Pin actual YieldBasis LEVAMM state, fee, wallet inventory and quotes including the entire-debt boundary."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode,encode
from eth_utils import keccak

from record_balancer_markets import Capture,posts
from record_balancer_v1 import fingerprint,rpc
from record_base_slipstream_executor import ROOT

FACTORY="0x370a449febb9411c95bf897021377fe0b7d100c0"
MULTICALL="0xca11bde05977b3631167028862be2a173976ca11"


def record(url):
    chain=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c=Capture(dict(chain,endpoint=url))
    anchor=rpc(url,"eth_getBlockByNumber",[hex(chain["block"]),False])
    if anchor["hash"]!=c.selector["blockHash"] or rpc(url,"eth_chainId",[])!="0x1":
        raise ValueError("wrong chain or canonical anchor")
    count,=c.one(FACTORY,"market_count()",[],[],["uint256"])
    stable,=c.one(FACTORY,"STABLECOIN()",[],[],["address"])
    registered=posts(c,[(FACTORY,"markets(uint256)",["uint256"],[i])for i in range(count)])
    rows=[]
    for i,v in enumerate(registered):
        asset,pool,amm,lt,oracle,_,_=decode(["address"]*7,v)
        requests=[(amm,"get_state()",[],[]),(oracle,"price()",[],[]),(amm,"fee()",[],[]),
                  (amm,"LEVERAGE()",[],[]),(amm,"is_killed()",[],[]),(pool,"decimals()",[],[]),
                  (stable,"balanceOf(address)",["address"],[amm]),(pool,"balanceOf(address)",["address"],[amm])]
        state,price,fee,leverage,killed,dec,stables,physical=posts(c,requests)
        collateral,debt,x0=decode(["uint256"]*3,state)
        sizes=[(side,qty)for side,base in ((0,debt),(1,collateral))for qty in (max(1,base//10**5),max(1,base//10**3),base)]
        packed=[(amm,True,keccak(text="get_dy(uint256,uint256,uint256)")[:4]+encode(["uint256"]*3,[side,1-side,qty]))for side,qty in sizes]
        results,=c.one(MULTICALL,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[packed],["(bool,bytes)[]"])
        quotes=[dict(side=side,qty=str(qty),out=str(int.from_bytes(raw,"big")) if ok else None)for (side,qty),(ok,raw) in zip(sizes,results,strict=True)]
        rows.append(dict(index=i,asset=asset,pool=pool,amm=amm,lt=lt,oracle=oracle,stable=stable,
                         runtime=fingerprint(url,amm,c.selector)["keccak256"], collateral=str(collateral),
                         debt=str(debt),x0=str(x0),price=str(int.from_bytes(price,"big")),fee=str(int.from_bytes(fee,"big")),
                         leverage=str(int.from_bytes(leverage,"big")),decimals=int.from_bytes(dec,"big"),
                         killed=bool(int.from_bytes(killed,"big")),physical=list(map(str,(int.from_bytes(stables,"big"),int.from_bytes(physical,"big")))),
                         quotes=quotes))
    return dict(block=chain["block"],selector=c.selector,timestamp=int(anchor["timestamp"],16),factory=FACTORY,rows=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("out",type=Path);parser.add_argument("--rpc",required=True)
    args=parser.parse_args()
    args.out.write_bytes(gzip.compress(json.dumps(record(args.rpc),separators=(",",":")).encode(),mtime=0))
