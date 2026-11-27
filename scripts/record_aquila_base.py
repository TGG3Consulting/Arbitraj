"""Compare Aquila's deployed Base router with V5 on the same funded pool and parent."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import OWNER, ROOT, rpc, simulate_block
from record_bitswap_ethereum import balance, tx
from record_omni_v2_base import ENDPOINTS, WETH, role
from record_v2_chain_pools import measure

ROUTER = "0x109074b31759a5b08164e54c44333f33247e7fd1"


def capture(endpoint):
    fixture = next(row for row in json.loads((ROOT/"tests/data/base_ethereum_v2_probes.json").read_bytes())
                   if row["network"] == "base" and row["dex"] == "rubicon-base")
    snap = json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    chain = json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())["base"]
    c = Capture(dict(snap,endpoint=endpoint))
    head = rpc(endpoint,"eth_getBlockByHash",[chain["hash"],False])
    if head["hash"] != c.selector["blockHash"] or fixture["block_hash"] != head["hash"]:
        raise ValueError("different Base state")
    spec, router = role(c,ROUTER)
    if spec["compilation"]["name"] != "AquilaRouter" or c.one(ROUTER,"factory()",returns=["address"])[0] != fixture["factory"] or c.one(ROUTER,"WETH()",returns=["address"])[0] != WETH:
        raise ValueError("router belongs to another family")
    # Re-run both V5 controls first. The same amounts then go through the deployed public router.
    v5 = measure(fixture,chain,endpoint)
    other = next(t for t in fixture["tokens"] if t != WETH)
    deadline = int(head["timestamp"],16) + 120
    wallet = {OWNER: {"balance":hex(100*10**18)}}
    check = [balance(WETH,OWNER),balance(other,OWNER)]
    cases = []
    for row in v5["cases"]:
        amount,bought,sold = [int(row[k]) for k in ("in","bought","returned")]
        path = [tx(WETH,"deposit()",value=amount)]
        for source,target,qty,out in ((WETH,other,amount,bought),(other,WETH,bought,sold)):
            def trade(bound,when):
                return tx(ROUTER,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                          ["uint256","uint256","address[]","address","uint256"],[qty,bound,[source,target],OWNER,when])
            before = path + [tx(source,"approve(address,uint256)",["address","uint256"],[ROUTER,qty])]
            quote = tx(ROUTER,"getAmountsOut(uint256,address[])",["uint256","address[]"],[qty,[source,target]])
            calls = before + [quote,*check,trade(out+1,deadline),trade(out,0),trade(out,deadline),*check]
            rows = simulate_block(endpoint,c.selector,wallet,calls)
            if any(x["status"]!="0x1" for x in rows[:-5]+rows[-3:]) or any(x["status"]!="0x0" for x in rows[-5:-3]):
                raise ValueError("router payout or stricter control changed")
            quoted, = decode(["uint256[]"],bytes.fromhex(rows[-8]["returnData"][2:]))
            if tuple(quoted)!=(qty,out):raise ValueError("router quote differs from the V5 cash flow")
            old,new = ([int(x["returnData"],16) for x in v] for v in (rows[-7:-5],rows[-2:]))
            i=int(source!=WETH)
            if (old[i]-new[i],new[1-i]-old[1-i])!=(qty,out):raise ValueError("wallet balance change mismatch")
            path = before + [trade(out,deadline)]
            cases.append(dict(source=source,target=target,spent=str(qty),received=str(out),gas=int(rows[-3]["gasUsed"],16)))
    return dict(selector=c.selector,router=router,pool=fixture["pool"],v5=v5["cases"],cases=cases)


if __name__ == "__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output",type=Path)
    args=cli.parse_args()
    for endpoint in ENDPOINTS:
        try:
            evidence=capture(endpoint)
            break
        except (requests.RequestException,RuntimeError) as error:
            print(endpoint,str(error)[:160],flush=True)
    else:
        raise RuntimeError("no archive node available")
    args.output.write_text(json.dumps(evidence,separators=(",",":")),encoding="utf8")
