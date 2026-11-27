"""Read same-hash ERC20/native vault balances and full liquidity tuples for the paid Bancor V3 markets."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode
from record_balancer_markets import Capture, posts

ROOT=Path(__file__).resolve().parent.parent / "tests/data/first_run_dex_2026-11-26"
NATIVE="0x"+"ee"*20


def read(endpoint):
    original=json.loads(gzip.decompress((ROOT/"bancor_v3_ethereum.json.gz").read_bytes()))
    c=Capture(dict(endpoint=endpoint,selector=original["selector"]))
    def rpc(method,params):
        if method not in {"eth_getBlockByHash","eth_chainId","eth_getBalance"}:
            raise ValueError(method)
        r=c.session.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=15)
        r.raise_for_status()
        return r.json()["result"]
    block=rpc("eth_getBlockByHash",[c.selector["blockHash"],False])
    if int(rpc("eth_chainId",[]),16)!=1 or block["hash"]!=c.selector["blockHash"] or int(block["number"],16)!=original["block"]:
        raise ValueError("unexpected chain or block")
    vault=original["roles"]["MasterVault"]["address"]
    active=sorted(t for t,p in original["pools"].items() if p["enabled"])
    specs=[]
    for t in active:
        col=original["pools"][t]["collection"]
        specs.extend([(col,"poolLiquidity(address)",["address"],[t]),(col,"networkFeePPM()",[],[])])
    results=posts(c,specs)
    pools={}
    for i,t in enumerate(active):
        bnt,base,staked=decode(["uint128","uint128","uint256"],results[i*2])
        if original["pools"][t]["base"]!=str(base) or original["pools"][t]["bnt"]!=str(bnt):
            raise ValueError("trading liquidity changed at pinned hash")
        pools[t]={"staked":str(staked),"network_fee":decode(["uint32"],results[i*2+1])[0]}
    erc=sorted(set(active+[original["bnt"]])-{NATIVE})
    raw=posts(c,[(t,"balanceOf(address)",["address"],[vault]) for t in erc])
    balances={t:str(decode(["uint256"],x)[0]) for t,x in zip(erc,raw,strict=True)}
    balances[NATIVE]=str(int(rpc("eth_getBalance",[vault,c.selector]),16))
    return dict(selector=c.selector,vault=vault,pools=pools,balances=balances)


if __name__=="__main__":
    p=argparse.ArgumentParser()
    p.add_argument("--endpoint",required=True)
    args=p.parse_args()
    result=read(args.endpoint)
    (ROOT/"bancor_v3_liquidity.json").write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
