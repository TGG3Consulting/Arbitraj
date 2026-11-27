"""Запись настоящих tick window и ответов Quoter для сверки Rust-математики двух сетей."""
import argparse
import gzip
import json
from pathlib import Path

from record_balancer_markets import Capture
from record_maverick_two_directory import DATA, ENDPOINTS, QUOTER, LENS

TICKS="(uint128,uint128,uint128,uint32[4])[]"
# These archival windows are measurement settings for the fixtures.
RADIUS=32


def record(network,address):
    with gzip.open(DATA/f"maverick_v2_{network}_directory.json.gz","rt") as f:directory=json.load(f)
    m=directory["markets"][address]
    c=Capture(dict(endpoint=ENDPOINTS[network],selector=directory["selector"]))
    active=m["active_tick"]
    scales=[c.one(address,n+"()",returns=["uint256"])[0] for n in ["tokenAScale","tokenBScale"]]
    indices,states=c.one(LENS,"getTicksAroundActive(address,int32)",["address","int32"],[address,RADIUS],["int32[]",TICKS])
    ticks=[dict(tick=i,reserveA=str(a),reserveB=str(b),totalSupply=str(s)) for i,(a,b,s,_bins) in zip(indices,states,strict=True)]
    q=[]
    for a_in in [True,False]:
        for exact_out in [False,True]:
            asset=int((not a_in) != exact_out)
            one=10**18//scales[asset]
            for qty in [max(1,one//10000),max(2,one//1000)]:
                # calculateSwap simulates a child swap and reverts its writes inside eth_call.
                maximum=2**31-1 if a_in else -2**31
                args=[address,qty,a_in,exact_out,maximum]
                used,paid,gas=c.one(QUOTER,"calculateSwap(address,uint128,bool,bool,int32)",
                    ["address","uint128","bool","bool","int32"],args,["uint256","uint256","uint256"])
                q.append(dict(tokenAIn=a_in,exactOutput=exact_out,amount=str(qty),amountIn=str(used),amountOut=str(paid),gas=str(gas)))
    return dict(network=network,pool=address,selector=directory["selector"],tokens=m["tokens"],tickRadius=RADIUS,
        activeTick=active,reserveA=m["reserves"][0],reserveB=m["reserves"][1],protocolFeeRatioD3=m["protocol_ratio_d3"],
        tickSpacing=str(m["tick_spacing"]),feeAIn=m["fees_d18"][0],feeBIn=m["fees_d18"][1],tokenAScale=str(scales[0]),
        tokenBScale=str(scales[1]),ticks=ticks,quotes=q)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);args=p.parse_args()
    chosen={"base":["0xdf033790907c60c9b81ae355f76f74f52f92114a","0xb6e91d9b24092d6cf422c37aff8ccaa9f95abe4e"],
            "ethereum":["0xeb1da432d5c1a9fdf52aa5d37698f34706f91397","0xc2f2ad178a1f07d349cf8349b74fce3cb69975de"]}
    rows=[record(n,a) for n,markets in chosen.items() for a in markets]
    args.output.write_text(json.dumps({"pools":rows},indent=2)+"\n",encoding="utf8",newline="\n")
