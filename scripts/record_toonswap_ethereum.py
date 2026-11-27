"""Replay ToonSwap's actual wallet debits and validate both fee transfers against Sync."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import rpc
from record_bitswap_ethereum import BLOCK, OWNER, tx
from record_concave_ethereum import wallet_cases
from record_ethereum_v2_factories import WETH, digest, raw, registry
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH, verified
from record_v2_chain_pools import swap

FACTORY = "0xe71205b53e2d48d8972b2e259028d83e24ae2a6b"
ROUTER = "0x7481313e8a71437b67d9dd03ae38c5ab1531e0ae"
DOC = "https://toonswaps-organization.gitbook.io/toonswap/support/smart-contracts"
SWAP = "swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)"


def directory(c):
    _, f, factory_code = verified(c, FACTORY, "ToonswapFactory")
    _, _, router_code = verified(c, ROUTER, "ToonswapRouter")
    origin = next(src["content"] for name, src in f["sources"].items() if name.endswith("/Pair.sol"))
    normalized = "".join(origin.split())
    for expression in ("balance0.mul(10000).sub(amount0In.mul(25))", "balance1.mul(10000).sub(amount1In.mul(25))",
                       "_update0(amount0In*25/10000)", "_update1(amount1In*25/10000)",
                       "_safeTransfer(token0,_dibs,_referralFee)", "_safeTransfer(token1,_treasury,treasuryAmt)"):
        if expression not in normalized:
            raise ValueError("deployed fee accounting differs: "+expression)
    init, = c.one(FACTORY, "INIT_CODE_PAIR_HASH()", returns=["bytes32"])
    if (c.one(ROUTER, "factory()", returns=["address"]) != (FACTORY,)
            or c.one(ROUTER, "WETH()", returns=["address"]) != (WETH,)):
        raise ValueError("router binding differs")
    roles = {"factory": FACTORY, "router": ROUTER}
    for key in ["dibs", "treasury"]:
        roles[key], = c.one(FACTORY, key+"()", returns=["address"])
    roles["mining"], = c.one(ROUTER, "swapMining()", returns=["address"])
    rates = [c.one(FACTORY, method+"()", returns=["uint256"])[0] for method in ["REFERRAL_FEE", "TREASURY_FEE"]]
    markets = registry(c, FACTORY, "0x"+init.hex())
    for m in markets:
        m["runtime"] = digest(raw(rpc(c.endpoint, "eth_getCode", [m["pool"], c.selector])))
        m["balances"] = [str(c.one(t, "balanceOf(address)", ["address"], [m["pool"]], ["uint256"])[0]) for t in m["tokens"]]
    if len({m["runtime"] for m in markets}) != 1:
        raise ValueError("multiple pair implementations in same CREATE2 registry")
    return dict(roles=roles, factory_runtime=digest(factory_code), router_runtime=digest(router_code),
                pair_init="0x"+init.hex(), rates=rates, markets=markets)


def capture(endpoint, payments):
    tag = {"blockHash": BLOCK, "requireCanonical": True}
    c = Capture(dict(endpoint=endpoint, selector=tag))
    head = rpc(endpoint, "eth_getBlockByHash", [BLOCK, False])
    if rpc(endpoint, "eth_chainId", []) != "0x1" or head["hash"] != BLOCK or head["stateRoot"] != ROOT_HASH:
        raise ValueError("network or canonical state differs")
    data = directory(c)
    data.update(selector=tag, state_root=ROOT_HASH, height=int(head["number"], 16), source=DOC)
    if not payments:
        return data
    def direct(a, b, n, minimum, end):
        return tx(ROUTER, SWAP, ["uint256","uint256","address[]","address","uint256"], [n,minimum,[a,b],OWNER,end])
    cases, failures = wallet_cases(endpoint, tag, int(head["timestamp"], 16)+200, data["markets"], ROUTER, direct,
                                  lambda m,a,b,n,v,e: swap(m["pool"],a,b,n,v,e,2500))
    if not cases:
        raise ValueError(("no measured payments", failures[:2]))
    topics = { "0x"+keccak(text=sig).hex(): key for key,sig in
              [("sync","Sync(uint112,uint112)"),("swap","Swap(address,uint256,uint256,uint256,uint256,address)"),
               ("referral","ReferralFees(address,uint256)"),("treasury","TreasuryFees(address,uint256)")] }
    for case in cases:
        logged = {}
        for log in case["logs"]:
            kind = topics.get(log["topics"][0]) if log["topics"] else None
            if log["address"].lower() != case["pool"] or kind is None:
                continue
            if kind in logged:
                raise ValueError(("repeated target event",kind))
            count = {"sync":2, "swap":4, "referral":1, "treasury":1}[kind]
            logged[kind] = decode(["uint256"]*count, raw(log["data"]))
        m = next(m for m in data["markets"] if m["pool"] == case["pool"])
        i = m["tokens"].index(case["source"])
        dx, dy = logged["swap"][i], logged["swap"][2+1-i]
        fee = dx*25//10000
        removed = [fee*part//10000 for part in data["rates"]]
        if tuple(x[0] for x in (logged["referral"], logged["treasury"])) != tuple(removed):
            raise ValueError(("fee floors",case["pool"]))
        reserves = list(map(int, case["reserves"]))
        expected = reserves.copy()
        expected[i] += dx - sum(removed)
        expected[1-i] -= dy
        case.update(physical_input=str(dx), physical_output=str(dy), fee_transfers=list(map(str,removed)),
                    post_reserves=list(map(str,logged["sync"])), reserve_model_matches=list(logged["sync"])==expected)
    return dict(data, cases=cases, unavailable=failures)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=["directory","payments"])
    parser.add_argument("output",type=Path)
    args = parser.parse_args()
    errors = []
    for endpoint in ENDPOINTS:
        try:
            report = capture(endpoint, args.stage == "payments")
            args.output.write_text(json.dumps(report,separators=(",",":"))+"\n",encoding="utf8")
            break
        except (ValueError, StopIteration, requests.RequestException, RuntimeError, KeyError) as err:
            errors.append((endpoint,str(err)[:250]))
    else:
        raise ValueError(errors)
