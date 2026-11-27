"""Measure real wallet USDT↔WETH across Curve's crvUSD bridge and each independent YieldBasis WETH venue."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_balancer_v1 import fingerprint, rpc
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import balance, tx, token_flow
from record_ethereum_v2_factories import FUNDING, STABLES, WETH
from record_safeswap_protected import runtime
from record_yield_basis_cash import UINT, approve
from record_yield_basis_wallet import MODULE, ROLES

# Curve's published Ethereum crvUSD/USDT pool, cross-check both coin positions at H.
SOURCE = "https://www.curve.finance/dex/ethereum/pools/0x390f3595bCa2Df7d23783dFd126427CCeb997BF4"
BRIDGE = "0x390f3595bca2df7d23783dfd126427cceb997bf4"
USDT = STABLES[1]


def capture(endpoint, artifact, book):
    at = book["selector"]
    head = rpc(endpoint, "eth_getBlockByHash", [at["blockHash"], False])
    if rpc(endpoint, "eth_chainId", []) != "0x1" or int(head["number"], 16) != book["block"]:
        raise ValueError("foreign canonical chain or block")
    ts = int(head["timestamp"],16)
    c = Capture(dict(endpoint=endpoint,selector=at))
    stable = book["stable"]
    coins = [c.one(BRIDGE, "coins(uint256)", [UINT], [i], ["address"])[0] for i in range(2)]
    if coins != [USDT, stable]:
        raise ValueError(("bridge currencies", coins))
    code = runtime(c, artifact, [book["factory"], WETH])
    word = lambda n: "0x"+format(n,"064x")
    overrides = {OWNER:{"balance":hex(10**18)}, MODULE:{"code":code},
                 EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                       "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    if any(rpc(endpoint,"eth_getCode",[a,at])!="0x" for a in overrides):
        raise ValueError("test identity in use")
    assets = [USDT,stable,WETH]
    wallets = [OWNER,EXEC,MODULE]
    cash = [balance(t,w) for w in wallets for t in assets]
    rows=[]
    for venue,entry in book["venues"].items():
        if entry["kind"]=="amm" or entry["coins"]!=[stable,WETH]:
            continue
        index=entry["markets"][0]
        packed=(index<<168)|(ROLES.index(entry["kind"])<<160)|int(venue,16)
        for sell in (False,True):
            src,dst=(WETH,USDT) if sell else (USDT,WETH)
            yb=(17,MODULE,WETH if sell else stable,stable if sell else WETH,10000,packed.to_bytes(32,"big"))
            cp=(4,BRIDGE,stable if sell else USDT,USDT if sell else stable,10000,encode(["int128","int128"],[int(sell),int(not sell)]))
            steps=[yb,cp] if sell else [cp,yb]
            for amount in ((10**15,5*10**15) if sell else (2_000_000,10_000_000)):
                prep=([tx(WETH,"deposit()",value=amount)] if sell else
                      [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",[UINT,"address[]","address",UINT],[amount,[WETH,USDT],OWNER,ts+600],value=10**17)])
                prep.append(approve(src,EXEC,amount))
                def order(minimum,expiry):
                    call=tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                            ["address",UINT,"address",UINT,UINT,STEP+"[]"],[src,amount,dst,minimum,expiry,steps])
                    call["gas"]=hex(8_000_000)
                    return call
                def trial(orders):
                    frame=simulate_block(endpoint,at,overrides,prep+cash+orders+cash,rpc)
                    b=len(prep)+len(cash)
                    if any(r["status"]!="0x1" for r in frame[:b]+frame[b+len(orders):]):
                        raise ValueError(("preparation/balances",venue,sell,amount))
                    before=[int(r["returnData"],16) for r in frame[len(prep):b]]
                    after=[int(r["returnData"],16) for r in frame[b+len(orders):]]
                    return frame[b:b+len(orders)], [y-x for x,y in zip(before,after,strict=True)], frame[len(prep)-1]
                trades,deltas,approval=trial([order(1,ts+600)])
                if trades[0]["status"]!="0x1":
                    raise ValueError(("composed route rejected",venue,sell,amount,trades[0].get("error")))
                out=deltas[assets.index(dst)]
                if out<=0 or deltas[assets.index(src)]!=-amount or deltas[1]!=0 or any(deltas[3:]):
                    raise ValueError(("unaccounted balance",venue,deltas))
                bound,verified,_=trial([order(out+1,ts+600),order(1,0),order(out,ts+600)])
                if [r["status"] for r in bound]!=["0x0","0x0","0x1"] or verified!=deltas:
                    raise ValueError(("limit rollback",venue,deltas,verified))
                trade=bound[-1];logs=trade["logs"]
                bridge_delta=token_flow(logs,stable,BRIDGE)
                if (bridge_delta<=0 if sell else bridge_delta>=0) or token_flow(logs,USDT,BRIDGE)!=( -out if sell else amount):
                    raise ValueError(("bridge settlement",venue,bridge_delta))
                rows.append(dict(index=index,venue=venue,role=entry["kind"],source=src,target=dst,spent=str(amount),received=str(out),
                                 bridge_crvusd_raw=str(abs(bridge_delta)),wallet_deltas=list(map(str,deltas)),gas=int(trade["gasUsed"],16),
                                 approval_gas=int(approval["gasUsed"],16),steps=[q[0] for q in steps],limits=[r["status"] for r in bound],logs=logs))
    if not rows or not all(any(r["role"]==k and r["source"]==t for r in rows) for k in ("virtual_pool","cryptopool") for t in (USDT,WETH)):
        raise ValueError("missing physical leg")
    return dict(block=book["block"],selector=at,bridge=BRIDGE,source=SOURCE,coins=coins,bridge_code=fingerprint(endpoint,BRIDGE,at),
                module_runtime="0x"+keccak(hexstr=code).hex(),delta_order=assets,holders=wallets,rows=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiled",type=Path);parser.add_argument("out",type=Path)
    args=parser.parse_args()
    book=json.loads(gzip.decompress((ROOT/"tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz").read_bytes()))
    errors=[]
    for node in ("https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"):
        try:
            measured=capture(node,json.loads(args.compiled.read_bytes()),book);break
        except (ValueError,KeyError,requests.RequestException,RuntimeError) as e:
            errors.append((node,str(e)[:400]))
    else:raise RuntimeError(errors)
    args.out.write_text(json.dumps(measured,separators=(",",":")),encoding="utf8")
