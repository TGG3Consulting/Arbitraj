"""Supplement a verified EtherVista wallet archive with both contract fees for every registered market."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from record_base_slipstream_executor import calldata
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import rpc
from record_ethervista_wallets import NETWORKS


def balances(c,pools):
    calls=[(token,True,bytes.fromhex(calldata("balanceOf(address)",["address"],[row["pool"]])[2:]))
           for row in pools for token in row["tokens"]]
    values=[]
    for i in range(0,len(calls),64):
        group=calls[i:i+64]
        out,=c.one("0xca11bde05977b3631167028862be2a173976ca11","aggregate3((address,bool,bytes)[])",
                   ["(address,bool,bytes)[]"],[group],["(bool,bytes)[]"])
        assert len(out)==len(group)
        values.extend(str(decode(["uint256"],data)[0]) if ok and len(data)==32 else None for ok,data in out)
    amounts=iter(values)
    return [[next(amounts),next(amounts)] for _ in pools]


def capture(snap,endpoint):
    c=Capture(dict(endpoint=endpoint,selector=snap["selector"]))
    head=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    chain=8453 if snap["network"]=="base" else 1
    assert int(rpc(endpoint,"eth_chainId",[]),16)==chain and int(head["number"],16)==snap["block"] and head["stateRoot"]==snap["state_root"]
    pools=snap["registry"]
    operations=[(p["pool"],fn,[],[]) for p in pools for fn in ("buyTotalFee()","sellTotalFee()")]
    amounts=iter(decode(["uint8"],b)[0] for b in posts(c,operations))
    terms=[(next(amounts),next(amounts)) for _ in pools]
    denominations=sorted(set(v for row in terms for v in row))
    ops=[(snap["router"],"usdcToEth(uint256)",["uint256"],[v]) for v in denominations]
    wei={str(unit):str(decode(["uint256"],raw)[0]) for unit,raw in zip(denominations,posts(c,ops),strict=True)}
    for p,fee,cash in zip(pools,terms,balances(c,pools),strict=True):
        p["fee_units"]=fee
        p["balances"]=cash
    snap["fee_wei_per_units"]=wei
    verified=snap["verified"]
    for address,row in verified.items():
        raw=rpc(c.endpoint,"eth_getCode",[address,c.selector])
        row["bytes"]=(len(raw)-2)//2
    return snap


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("archive",type=Path)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    archive=json.loads(args.archive.read_bytes())
    errors=[]
    for endpoint in NETWORKS[archive["network"]][-1]:
        try:
            data=capture(archive,endpoint)
            args.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf-8",newline="\n")
            break
        except (ValueError,KeyError,AssertionError,OSError) as e:
            errors.append((endpoint,str(e)[:160]))
    else:
        raise ValueError(f"no archive returned fees on the accepted header: {errors}")
