"""Record the executable and rejected Sobal quotes alongside the precise input to Rust curve pricing."""
import argparse
import json
from itertools import permutations
from pathlib import Path

from eth_abi import decode
from record_balancer_markets import Capture, ZERO, posts
from record_base_slipstream_executor import ROOT
from record_sobal_directory import VAULT

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
SWAPS = "(bytes32,uint256,uint256,uint256,bytes)[]"
FUND = "(address,bool,address,bool)"


def capture(directory):
    reader = Capture(dict(endpoint="https://mainnet.base.org",selector=directory["selector"]))
    result = []
    for market in directory["pools"]:
        families = market["families"]
        if not families:
            continue
        assert len(families) == 1
        tokens = market["tokens"]
        pool = market["pool"]
        kind, = families
        balances = list(map(int, market["balances"]))
        if not any(q for t,q in zip(tokens, balances) if t != pool):
            continue
        decimals = [int.from_bytes(raw,"big") for raw in posts(reader, [(t,"decimals()",[],[]) for t in tokens])]
        swap_fee, = reader.one(pool,"getSwapFeePercentage()",returns=["uint256"])
        pause = reader.one(pool,"getPausedState()",returns=["bool","uint256","uint256"])
        attrs = dict(nonce=market["nonce"], id=market["id"], pool=pool, tokens=tokens, balances=market["balances"],
                     decimals=decimals, fee=str(swap_fee), paused=pause[0], kind=kind)
        if kind == "weighted":
            weights, = reader.one(pool,"getNormalizedWeights()",returns=["uint256[]"])
            attrs["weights"] = list(map(str, weights))
        elif kind == "composable_stable":
            amp, updating, precision = reader.one(pool,"getAmplificationParameter()",returns=["uint256","bool","uint256"])
            factors, = reader.one(pool,"getScalingFactors()",returns=["uint256[]"])
            bpt, = reader.one(pool,"getBptIndex()",returns=["uint256"])
            assert tokens[bpt] == pool and precision == 1000
            attrs.update(amp=str(amp),updating=updating,bpt=bpt,factors=list(map(str,factors)))
        else:
            raise ValueError("funded managed market requires its circuit breaker and actual supply")
        cases = []
        for i,j in permutations(range(len(tokens)),2):
            if pool in (tokens[i],tokens[j]) or not min(balances[i],balances[j]):
                continue
            sizes = sorted({max(1,balances[i]//10000),max(1,balances[i]//1000)})
            for n in sizes:
                op=(VAULT,"queryBatchSwap(uint8,"+SWAPS+",address[],"+FUND+")",["uint8",SWAPS,"address[]",FUND],
                    [0,[(bytes.fromhex(attrs["id"][2:]),0,1,n,b"")],[tokens[i],tokens[j]],(ZERO,False,ZERO,False)])
                data, = reader.batch([op])
                if data is None:
                    cases.append(dict(i=i,j=j,amount=str(n),error=reader.calls[-1]["response"]))
                    continue
                deltas, = decode(["int256[]"],data)
                if len(deltas)!=2 or deltas[0]!=n or deltas[1]>0:
                    raise ValueError("invalid query result")
                cases.append(dict(i=i,j=j,amount=str(n),output=str(-deltas[1])))
        attrs["cases"] = cases
        result.append(attrs)
    return dict(chain=8453, selector=directory["selector"], vault=VAULT, markets=result)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    directory=json.loads((DATA/"sobal_base_directory.json").read_bytes())
    args.output.write_text(json.dumps(capture(directory),indent=2)+"\n",encoding="utf8")
