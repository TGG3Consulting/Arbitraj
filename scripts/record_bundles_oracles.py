"""Capture LP supply and the oracle inputs required to reproduce Bundles Swap's physical fee."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from record_balancer_markets import Capture, posts
from record_bitswap_ethereum import read_batch
from record_ethereum_x7 import ENDPOINTS
from record_bundles_wallets import DATA


def capture(endpoint):
    data=json.loads(gzip.decompress((DATA/"bundles_ethereum_directory.json.gz").read_bytes()))
    at=data["selector"]
    chain,header=read_batch(endpoint,[("eth_chainId",[]),("eth_getBlockByHash",[at["blockHash"],False])])
    if chain!="0x1" or header["stateRoot"]!=data["state_root"] or header["hash"]!=at["blockHash"]:
        raise ValueError("unexpected canonical chain")
    c=Capture(dict(selector=at,endpoint=endpoint))
    pools=data["markets"]
    values=posts(c,[(m["pool"],"totalSupply()",[],[]) for m in pools])
    oracles=sorted({m["oracle"] for m in pools if int(m["oracle"],16)})
    fields=[("pair()","address"),("token1()","address"),("price1Average()","uint224"),
            ("price1CumulativeLast()","uint256"),("blockTimestampLast()","uint32"),("firstUpdateCall()","bool")]
    readings=iter(posts(c,[(o,method,[],[]) for o in oracles for method,_ in fields]))
    oracle_data={}
    for o in oracles:
        observed={k[:-2]:decode([t],next(readings))[0] for k,t in fields}
        if not any(m["pool"]==observed["pair"] and m["tokens"]==[data["protocol_token"],observed["token1"]]
                   and m["oracle"]==o and m["fee"]==10 for m in pools):
            raise ValueError(("unregistered oracle",o))
        quoted=c.one(o,"consult(address,uint256)",["address","uint256"],[observed["token1"],1<<112],["uint256"])[0]
        if quoted!=observed["price1Average"]:raise ValueError("price precision differs")
        for name in ("price1Average","price1CumulativeLast"):
            observed[name]=str(observed[name])
        oracle_data[o]=observed
    return dict(selector=at,state_root=data["state_root"],oracles=oracle_data,
                supply={m["pool"]:str(int.from_bytes(s)) for m,s in zip(pools,values,strict=True)})


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    for ep in ENDPOINTS:
        try:
            result=capture(ep)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            print(len(result["supply"]),len(result["oracles"]))
            break
        except (ValueError,RuntimeError,OSError) as error:print(str(error)[:160])
    else:raise SystemExit("unverified supply or fee oracle")
