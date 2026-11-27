"""Снимает исходные bitmap, liquidityNet и slot0 по контрольному хешу Deli CL. Только eth_call."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT

DATA=ROOT/"tests/data/first_run_dex_2026-11-26"


def capture():
    snap=json.loads((DATA/"base.json").read_bytes())
    c=Capture(dict(snap,endpoint="https://mainnet.base.org"))
    v4=next(p["settings"] for p in json.loads((ROOT/"config/first_run_dex.json").read_bytes()) if p["id"]=="uniswap-v4-base")
    view=v4["dex.state_view"]
    records=[]
    for init in json.loads((DATA/"deli_base_cl_initializations.json").read_bytes()):
        pool=bytes.fromhex(init["topics"][1][2:])
        _,spacing,_,_,_=decode(["uint24","int24","address","uint160","int24"],bytes.fromhex(init["data"][2:]))
        sqrt,tick,protocol,fee=c.one(view,"getSlot0(bytes32)",["bytes32"],[pool],["uint160","int24","uint24","uint24"])
        liquidity,=c.one(view,"getLiquidity(bytes32)",["bytes32"],[pool],["uint128"])
        center=tick//spacing//256
        words=list(range(center-2,center+3))
        packed=c.batch([(view,"getTickBitmap(bytes32,int16)",["bytes32","int16"],[pool,word]) for word in words])
        ticks=[(word*256+bit)*spacing for word,raw in zip(words,packed,strict=True)
               for bit in range(256) if int.from_bytes(raw,"big") & (1<<bit)]
        details=c.batch([(view,"getTickLiquidity(bytes32,int24)",["bytes32","int24"],[pool,it]) for it in ticks]) if ticks else []
        all_ticks=[]
        for it,raw in zip(ticks,details,strict=True):
            gross,net=decode(["uint128","int128"],raw);all_ticks.append([it,str(gross),str(net)])
        records.append(dict(id="0x"+pool.hex(),sqrt=str(sqrt),tick=tick,protocol=protocol,fee=fee,liquidity=str(liquidity),
                            spacing=spacing,words=[words[0],words[-1]],ticks=all_ticks))
    return dict(selector=c.selector,state_view=view,markets=records,calls=c.calls)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path)
    parser.parse_args().output.write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8",newline="\n")
