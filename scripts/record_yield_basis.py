"""Record each YieldBasis market, the distinct physical venues and original wallet outcomes."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_balancer_v1 import fingerprint, rpc
from record_base_slipstream_executor import ROOT
from record_yield_basis_cash import measure

FACTORY = "0x370a449febb9411c95bf897021377fe0b7d100c0"
SOURCE = "https://github.com/yield-basis/yb-core/tree/a1433a0c3d3887ecd2ea085992d041014cf98468"
SLOTS = ["asset","cryptopool","amm","lt","oracle","virtual_pool","staker"]


def capture(url):
    chain = json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    at = chain["selector"]
    c = Capture(dict(chain,endpoint=url))
    header = rpc(url,"eth_getBlockByNumber",[hex(chain["block"]),False])
    if rpc(url,"eth_chainId",[]) != "0x1" or header["hash"] != at["blockHash"]:
        raise ValueError("wrong chain or block")
    n, = c.one(FACTORY,"market_count()",[],[],["uint256"])
    stable, = c.one(FACTORY,"STABLECOIN()",[],[],["address"])
    flash, = c.one(FACTORY,"flash()",[],[],["address"])
    raw = posts(c,[(FACTORY,"markets(uint256)",["uint256"],[i]) for i in range(n)])
    markets = [dict(zip(SLOTS,decode(["address"]*7,r),strict=True)) for r in raw]
    venues = {}
    for index,market in enumerate(markets):
        for label in ("cryptopool","amm","virtual_pool"):
            address = market[label]
            if int(address,16)==0:
                continue
            if address in venues:
                venues[address]["markets"].append(index)
                continue
            coins = [decode(["address"],r)[0] for r in posts(c,[(address,"coins(uint256)",["uint256"],[i]) for i in range(2)])]
            if coins != [stable, market["cryptopool"] if label=="amm" else market["asset"]]:
                raise ValueError(("coins",index,label,coins))
            tick = [int.from_bytes(r,"big") for r in posts(c,[(token,"decimals()",[],[]) for token in coins])]
            entry = dict(kind=label,coins=coins,decimals=tick,code=fingerprint(url,address,at),markets=[index])
            if label=="amm":
                roles = posts(c,[(address,sig,[],[]) for sig in
                                 ("LT_CONTRACT()","COLLATERAL()","STABLECOIN()","PRICE_ORACLE_CONTRACT()","is_killed()")])
                if [decode(["address"],r)[0] for r in roles[:4]]!=[market["lt"],market["cryptopool"],stable,market["oracle"]]:
                    raise ValueError(("amm roles",index))
                entry["killed"] = decode(["bool"],roles[4])[0]
            if label=="virtual_pool":
                roles = posts(c,[(address,sig,[],[]) for sig in ("FACTORY()","AMM()","POOL()","ASSET_TOKEN()","STABLECOIN()","IMPL()")])
                decoded = [decode(["address"],r)[0] for r in roles]
                if decoded[:5]!=[FACTORY,market["amm"],market["cryptopool"],market["asset"],stable]:
                    raise ValueError(("virtual roles",index))
                entry["impl"] = decoded[5]
            venues[address]=entry
    flash_capacity, = c.one(flash,"maxFlashLoan(address)",["address"],[stable],["uint256"])
    flash_fee, = c.one(flash,"flashFee(address,uint256)",["address","uint256"],[stable,flash_capacity],["uint256"])
    for addr,entry in venues.items():
        market=markets[entry["markets"][0]]
        entry["cases"] = measure(c,rpc,addr,entry,market,stable,int(header["timestamp"],16))
    if not any("spent" in r for v in venues.values() for r in v["cases"]):
        raise ValueError("no physical payment")
    return dict(chain_id=1,block=chain["block"],selector=at,source=SOURCE,factory=FACTORY,
                factory_code=fingerprint(url,FACTORY,at),stable=stable,flash=flash,flash_capacity=str(flash_capacity),
                flash_fee=str(flash_fee),markets=markets,venues=venues)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    failures=[]
    for url in ("https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"):
        try:
            data=capture(url)
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as exc:
            failures.append((url,str(exc)[:500]))
    else:
        raise RuntimeError(failures)
    args.out.write_bytes(json.dumps(data,separators=(",",":")).encode())
