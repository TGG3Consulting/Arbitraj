"""Сверяет все рынки OasisSwap, fee cache и закрытый слот MEV controller с исходником размещения."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture,posts
from record_base_slipstream_executor import OWNER,ROOT,rpc
from record_oasisswap_base import FACTORY,ROUTER


def capture(source):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    c=Capture(dict(snap,endpoint="https://mainnet.base.org"))
    assert source["selector"]==c.selector
    url=f"https://sourcify.dev/server/v2/contract/8453/{FACTORY}"
    response=requests.get(url,params={"fields":"storageLayout,runtimeBytecode.onchainBytecode"},timeout=25)
    response.raise_for_status()
    origin=response.json()
    code=rpc(c.endpoint,"eth_getCode",[FACTORY,c.selector])
    assert code.lower()==origin["runtimeBytecode"]["onchainBytecode"].lower()
    slot,=[v for v in origin["storageLayout"]["storage"] if v["label"]=="mevController"]
    assert slot["offset"]==0 and slot["type"]=="t_address"
    req={"id":1,"jsonrpc":"2.0","method":"eth_getStorageAt","params":[FACTORY,hex(int(slot["slot"])),c.selector]}
    storage=requests.post(c.endpoint,json=req,timeout=18)
    storage.raise_for_status()
    word=bytes.fromhex(storage.json()["result"][2:])
    assert len(word)==32 and not any(word[:12])
    markets=source["pools"]
    totals=iter(posts(c,[(token,"balanceOf(address)",["address"],[m["pool"]]) for m in markets for token in m["tokens"]]))
    for m in markets:
        amounts=[decode(["uint256"],next(totals))[0] for _ in range(2)]
        m["balances"]=list(map(str,amounts))
        m["balanced"]=[int(r)+int(cache) for r,cache in zip(m["reserves"],m["fee_cache"],strict=True)]==amounts
    r=source["origin"][FACTORY]
    assert r["runtime"]=="0x"+keccak(bytes.fromhex(code[2:])).hex()
    source["controls"]=dict(layout_url=url+"?fields=storageLayout",mev_slot=int(slot["slot"]),mev_controller="0x"+word[12:].hex(),
        estimator=c.one(FACTORY,"rebateEstimator()",returns=["address"])[0],
        approved=c.one(FACTORY,"isRebateApprovedRouter(address)",["address"],[ROUTER],["bool"])[0],
        rebate_at_sample=c.one(FACTORY,"getRebate(address)",["address"],[OWNER],["uint64"])[0])
    return source


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wallet_archive",type=Path)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    result=capture(json.loads(args.wallet_archive.read_bytes()))
    args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8",newline="\n")
