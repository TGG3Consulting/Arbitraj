"""Reconcile Ethereum Aquila's factory with its deployed pair and replay a funded fork."""
import argparse
import json
import os
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import OWNER, EXEC, ROOT, RUNTIME, rpc, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import FUNDING, STABLES, WETH, digest, raw, registry
from record_omni_v2_base import role
from record_v2_chain_pools import minimum, swap
from record_ranges_ethereum import ENDPOINT

FACTORY = "0x7bad585c3ae4ae266f92a4af13b388bc7b26067c"
ROUTER = "0x70d2473f911683fef696fb23ed25540e46d70734"
PAIR = "0x92356b1ca721ae1cf5bbed88a2528d87f311dff1"


def capture(endpoint):
    snap = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c = Capture(dict(snap, endpoint=endpoint))
    spec, factory = role(c, FACTORY, 1)
    router_source, router_binding = role(c, ROUTER, 1)
    router_factory, = c.one(ROUTER,"factory()",returns=["address"])
    router_wrapped, = c.one(ROUTER,"WETH()",returns=["address"])
    if (router_source["compilation"]["name"], router_factory, router_wrapped) != ("AquilaRouter", FACTORY, WETH):
        raise ValueError("router deployment does not bind this pair family")
    pair_source = next(s["content"] for f, s in spec["sources"].items() if f.endswith("/AquilaPair.sol"))
    for side in (0, 1):
        if f"balance{side}.mul(1000).sub(amount{side}In.mul(3))" not in pair_source:
            raise ValueError("Aquila swap invariant changed")
    url = "https://eth.blockscout.com/api/v2/smart-contracts/" + PAIR
    response = requests.get(url, timeout=30)
    response.raise_for_status()
    source = response.json()
    code = rpc(endpoint, "eth_getCode", [PAIR, c.selector])
    if not source["is_verified"] or source["is_changed_bytecode"] or source["deployed_bytecode"].lower() != code.lower():
        raise ValueError("verified pair differs from the pinned block")
    init = digest(raw(source["creation_bytecode"]))
    rows = registry(c, FACTORY, init)
    if len(rows) != 1 or rows[0]["pool"] != PAIR or rows[0]["tokens"] != [STABLES[0], WETH] or rows[0]["reserves"] != ["0", "0"]:
        raise ValueError("different Aquila market at canonical parent")
    head = rpc(endpoint, "eth_getBlockByHash", [c.selector["blockHash"], False])
    if int(rpc(endpoint, "eth_chainId", []), 16) != 1 or head["hash"] != c.selector["blockHash"]:
        raise ValueError("Ethereum parent mismatch")
    end = int(head["timestamp"], 16) + 900
    usdc = STABLES[0]
    assets = (usdc, WETH)
    reserves = (1_000 * 10**6, 10**18)
    word = lambda v: "0x" + format(v, "064x")
    overrides = {OWNER: {"balance": hex(100 * 10**18)}, EXEC: {
        "code": "0x" + RUNTIME.read_text().strip().removeprefix("0x"),
        "stateDiff": {word(0): word(int(OWNER,16)), word(1): word(int(WETH,16))}}}
    def run(calls):
        return simulate_block(endpoint, c.selector, overrides, calls)
    funding = [tx(FUNDING, "swapETHForExactTokens(uint256,address[],address,uint256)",
                  ["uint256","address[]","address","uint256"], [2*reserves[0],[WETH,usdc],OWNER,end], value=30*10**18),
               tx(WETH,"deposit()",value=2*reserves[1])]
    seed = [tx(a,"transfer(address,uint256)",["address","uint256"],[PAIR,r]) for a,r in zip(assets,reserves)]
    prefix = funding + seed + [tx(PAIR,"mint(address)",["address"],[OWNER])]
    probe = [tx(PAIR,"getReserves()")] + [balance(a,PAIR) for a in assets]
    state = run(prefix + probe)
    if any(x["status"] != "0x1" for x in state):
        raise ValueError(("isolated funding", [x.get("error") for x in state if x["status"] != "0x1"]))
    if decode(["uint112","uint112","uint32"],raw(state[-3]["returnData"]))[:2] != reserves or tuple(int(x["returnData"],16) for x in state[-2:]) != reserves:
        raise ValueError("reserves differ from cash after mint")
    cases = []
    for side in (0,1):
        a,b = assets[side],assets[1-side]
        for amount in (reserves[side]//10_000,reserves[side]//100):
            payout = minimum(amount,reserves[side],reserves[1-side],3000)
            quoted=run(prefix+[tx(ROUTER,"getAmountsOut(uint256,address[])",["uint256","address[]"],[amount,[a,b]])])[-1]
            if quoted["status"] != "0x1" or tuple(decode(["uint256[]"],raw(quoted["returnData"]))[0]) != (amount,payout):
                raise ValueError("Aquila router quote differs from pair invariant")
            for mode,spender in (("v5",EXEC),("router",ROUTER)):
                approved = tx(a,"approve(address,uint256)",["address","uint256"],[spender,amount])
                balances = [balance(token,wallet) for wallet in (OWNER,EXEC) for token in (a,b)]
                def order(out,when):
                    if mode == "v5":
                        return swap(PAIR,a,b,amount,out,when,3000)
                    return tx(ROUTER,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                              ["uint256","uint256","address[]","address","uint256"],[amount,out,[a,b],OWNER,when])
                attempts = run(prefix + [approved] + balances + [order(payout+1,end),order(payout,0),order(payout,end)] + balances)
                before, ops, after = attempts[-11:-7], attempts[-7:-4], attempts[-4:]
                if [op["status"] for op in ops] != ["0x0","0x0","0x1"] or any(x["status"]!="0x1" for x in attempts[:-7]+after):
                    raise ValueError(("settlement/protection", [op.get("error") for op in ops]))
                old,new=([int(k["returnData"],16) for k in row] for row in (before,after))
                if (old[0]-new[0],new[1]-old[1]) != (amount,payout) or old[2:] != new[2:]:
                    raise ValueError("wallet deltas disagree with V5 quote")
                cases.append(dict(mode=mode,token_in=a,token_out=b,amount=amount,payout=payout,gas=int(ops[-1]["gasUsed"],16),logs=ops[-1]["logs"]))
    return dict(selector=c.selector,block=snap["block"],state_root=head["stateRoot"],factory=factory,router=router_binding,init_hash=init,
                pair_runtime=digest(raw(code)),pair_source=url,live_rows=rows,isolated_reserves=reserves,cases=cases)


if __name__ == "__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output",type=Path)
    path=cli.parse_args().output
    # Архивный узел из секрета Actions ETH_ARCHIVE_RPC (если задан) — первым; адрес в выводе заменяется меткой.
    archive=os.environ.get("ETH_ARCHIVE_RPC","").strip()
    hide=lambda text: text.replace(archive,"ETH_ARCHIVE_RPC") if archive else text
    for endpoint in (archive,)*bool(archive)+(ENDPOINT,"https://ethereum-rpc.publicnode.com","https://eth.drpc.org"):
        try:
            data=capture(endpoint)
            break
        except (requests.RequestException,RuntimeError) as error:
            print(hide(endpoint), hide(str(error))[:180], flush=True)
    else:
        raise RuntimeError("no canonical archive available")
    path.write_text(json.dumps(data,separators=(",",":")),encoding="utf-8")
