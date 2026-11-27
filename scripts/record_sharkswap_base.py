"""Подтверждает происхождение Router02, весь реестр SharkSwap и обе кошельковые формы на Base."""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path

import requests
from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, rpc
from record_diamondswap_base import WETH, direct_wallet
from record_ethervista_wallets import tx
from record_ethereum_v2_factories import digest, raw, registry
from record_omni_v2_base import ENDPOINTS, role
from record_v2_chain_pools import measure

FACTORY = "0x57592d44eb60011500961ef177bff8d8691d5a8b"
ROUTER = "0x0573b0ce977bba12e31ad7a3ccc02d0db004d57a"
REFERENCE = "0xae45f7eb428eb6403f2567c3a3ec47e9230bc823"
USD = "0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca"


def capture(endpoint, wallets):
    chain = json.loads((ROOT / "tests/data/base_ethereum_networks.json").read_bytes())["base"]
    c = Capture({"endpoint":endpoint,"selector":{"blockHash":chain["hash"],"requireCanonical":True}})
    head = rpc(endpoint,"eth_getBlockByHash",[chain["hash"],False])
    if head["hash"] != chain["hash"] or int(head["number"],16)!=chain["block"] or int(rpc(endpoint,"eth_chainId",[]),16) != 8453:
        raise ValueError("wrong canonical Base block")
    roles, source = {}, {}
    for name, address in [("factory",FACTORY),("router",ROUTER),("pair",REFERENCE)]:
        record, binding = role(c,address)
        binding["deployment"] = record["deployment"]
        binding["name"] = record["compilation"]["fullyQualifiedName"]
        roles[name], source[name] = binding, record
    if roles["router"]["deployment"]["deployer"] != roles["factory"]["deployment"]["deployer"]:
        raise ValueError("factory and router deployer differ")
    init = digest(raw(source["pair"]["creationBytecode"]["onchainBytecode"]))
    router = source["router"]["sources"]["SharkswapRouter02.sol"]["content"]
    pair = source["pair"]["sources"]["SharkswapPair.sol"]["content"]
    if not all(s in router for s in [init[2:],"swapExactTokensForTokensSupportingFeeOnTransferTokens",
            "balanceOf(to).sub(balanceBefore) >= amountOutMin","deadline >= block.timestamp"]):
        raise ValueError("unconfirmed router code")
    if not all(s in pair for s in ["mul(1000).sub(amount0In.mul(3))","mul(1000).sub(amount1In.mul(3))","mul(1000**2)"]):
        raise ValueError("unconfirmed pair fee")
    if c.one(ROUTER,"factory()",returns=["address"]) != (FACTORY,) or c.one(ROUTER,"WETH()",returns=["address"]) != (WETH,):
        raise ValueError("wrong router roles")
    markets = registry(c,FACTORY,init)
    def runtime(market):
        return digest(raw(rpc(endpoint,"eth_getCode",[market["pool"],c.selector])))
    with ThreadPoolExecutor(max_workers=4) as workers:
        for market, code in zip(markets,workers.map(runtime,markets),strict=True):
            market["runtime"] = code
    fee_to, = c.one(FACTORY,"feeTo()",returns=["address"])
    data = dict(chain_id=8453,selector=c.selector,height=chain["block"],state_root=head["stateRoot"],roles=roles,
                init=init,swap_fee={"units":3,"denominator":1000},fee_to=fee_to,markets=markets,
                runtimes=dict(Counter(m["runtime"] for m in markets)),examples=[])
    if not wallets:
        return data
    def quote(src,dst,amount):
        return tx(ROUTER,"getAmountsOut(uint256,address[])",["uint256","address[]"],[amount,[src,dst]])
    selected = [m for m in markets if m["pool"]==REFERENCE or set(m["tokens"])=={USD,WETH}]
    if len(selected)!=2:
        raise ValueError("missing DAI/WETH or USDbC/WETH")
    for m in selected:
        if m["runtime"] != roles["pair"]["runtime"]:
            raise ValueError("unattested pair runtime")
        evidence = dict(network="base",dex="sharkswap-base",pool=m["pool"],tokens=m["tokens"],factory=FACTORY,
                        runtime_hash=m["runtime"],fee_fraction=[1000,3],verify_pair_boundary=True,invariant_error="Sharkswap: K")
        measured = measure(evidence,chain,endpoint=endpoint)
        amounts = [int(t["in"]) for t in measured["cases"]]
        cases = direct_wallet(c,m,amounts,int(head["timestamp"],16),3000,ROUTER,audit=True,quote_call=quote)
        for case in cases:
            for side in ["buy","sell"]:
                leg = case[side]
                quoted, = decode(["uint256[]"],raw(leg["official_quote"]["response"]))
                if tuple(quoted) != (int(leg["exact"]["spent"]),int(leg["exact"]["received"])):
                    raise ValueError("official quote and actual receipt differ")
        data["examples"].append(dict(pool=m["pool"],cases=cases,pair_boundary=measured))
    return data


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path)
    p.add_argument("--directory-only",action="store_true")
    args = p.parse_args()
    errors = []
    for endpoint in ENDPOINTS:
        try:
            result = capture(endpoint,not args.directory_only)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            break
        except (KeyError,ValueError,requests.RequestException,RuntimeError) as exc:
            errors.append((endpoint,str(exc)[:300]))
    else:
        raise ValueError(errors)
