"""Capture official pure spot quotes on every pair and both sides at the same Ethereum block."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from record_ammalgam_spot import FACTORY, HELPER
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, rpc

DIRECTORY = ROOT / "tests/data/first_run_dex_2026-11-26/ammalgam_ethereum_spot.json.gz"
QUOTE = "computeExpectedSwapAmountOut(uint256,uint256,uint256,uint256,uint256,uint256)"


def capture(url):
    book = json.loads(gzip.decompress(DIRECTORY.read_bytes()))
    selector = book["selector"]
    header = rpc(url,"eth_getBlockByHash",[selector["blockHash"], False])
    assert rpc(url,"eth_chainId",[])=="0x1" and header["stateRoot"]==book["state_root"]
    c = Capture(dict(selector=selector,endpoint=url))
    rows = []
    for market in book["pools"]:
        pool = market["pool"]
        reserves = list(map(int,market["reserves"]))
        timestamp = int(header["timestamp"],16)
        # ITokenController.getReserves and Pair.swap accrue on the same lending period.
        accrued = ((timestamp-reserves[2]) % 2**32) >= 24*60*60-1
        assets, _ = c.one(pool,"totalAssetsAndShares(bool)",["bool"],[accrued],["uint112[6]","uint112[6]"])
        missing = [max(assets[4+i]-assets[1+i],0) for i in range(2)]
        reference = c.one(pool,"referenceReserves()",returns=["uint112","uint112"])
        for i in (0,1):
            ix = 1-i
            state = [reserves[i],reference[i],reserves[ix],missing[i],missing[ix]]
            # Smallest unit, normal orders, fee transition, and the upper side of the quadratic region.
            amounts = sorted(a for a in {1,max(1,reserves[i]//10**8),max(1,reserves[i]//100000),
                              max(1,reserves[i]//100),max(1,reserves[i]//5),reserves[i],reserves[i]*2,
                              reference[i]-reserves[i],reference[i]-reserves[i]+1,
                              3*reference[i]-2*reserves[i],3*reference[i]-2*reserves[i]+1}
                             if 0<a<2**112-reserves[i])
            requests = [(HELPER,QUOTE,["uint256"]*6,[amount,*state]) for amount in amounts]
            values = []
            for pos in range(0,len(requests),5):
                values.extend(c.batch(requests[pos:pos+5]))
            assert len(values)==len(amounts)
            quotes = [{"amount":str(n),"quote":str(decode(["uint256"],v)[0]) if v is not None else None}
                      for n,v in zip(amounts,values)]
            rows.append(dict(pool=pool,input=market["tokens"][i],state=list(map(str,state)),accrued=accrued,
                             output_cash=market["cash"][ix],quotes=quotes))
    assert len(rows)==2*len(book["pools"])
    return dict(selector=selector,source=book["origin"],factory=FACTORY,helper=HELPER,
                implementation=book["pools"][0]["code"]["implementation"],rows=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    args.output.write_text(json.dumps(capture("https://rpc.mevblocker.io"),separators=(",",":")))
