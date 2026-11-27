"""Recover every Bundles Swap fee tier from the factory and bind both deployed pair ABIs."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import rpc
from record_bitswap_ethereum import BLOCK, read_batch
from record_ethereum_v2_factories import digest, raw
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH, verified

FACTORY = "0xacff9eee0a5522000e7141b77107359a6462e8d2"
ROUTER = "0x7088d2a3855bf69320869ea28e5d95a142935441"
ROUTER_RECEIPT = "0xa434a890f34e911ab33aea2854d257b96c13b2626b5ed9a8078c00a7614c920a"
REGISTRY_SOURCE = "https://github.com/DefiLlama/DefiLlama-Adapters/blob/main/projects/bundles/index.js"


def capture(endpoint):
    tag = {"blockHash": BLOCK, "requireCanonical": True}
    head = rpc(endpoint, "eth_getBlockByHash", [BLOCK, False])
    if rpc(endpoint, "eth_chainId", []) != "0x1" or head["hash"] != BLOCK or head["stateRoot"] != ROOT_HASH:
        raise ValueError("unexpected chain or state root")
    c = Capture(dict(endpoint=endpoint, selector=tag))
    source, _, factory_code = verified(c, FACTORY, "SwapFactory")
    protocol, = c.one(FACTORY, "protocolToken()", returns=["address"])
    fees, = c.one(FACTORY, "getPossibleFees()", returns=["uint16[6]"])
    total, = c.one(FACTORY, "allPairsLength()", returns=["uint256"])
    pools, cursor = c.one(FACTORY, "getPairs(uint256,uint256)", ["uint256"] * 2, [0,total], ["address[]","uint256"])
    if not total or cursor != total or len(pools) != total or len(set(pools)) != total:
        raise ValueError("incomplete factory directory")
    getters = [("token0()", ["address"]), ("token1()", ["address"]), ("factory()", ["address"]),
               ("getReserves()", ["uint112","uint112","uint32"]), ("fee()", ["uint16"]),
               ("protocolTokenReserve()", ["uint256"]), ("pairOracle()", ["address"])]
    values = iter(posts(c, [(p, key, [], []) for p in pools for key, _ in getters]))
    codes = read_batch(endpoint, [("eth_getCode", [p,tag]) for p in pools])
    rows, classes, lookups = [], {}, []
    for pool, code in zip(pools, codes, strict=True):
        a,b,parent,res,tier,held,oracle = [decode(types,next(values)) for _, types in getters]
        a,b,tier = a[0], b[0], tier[0]
        kind = "OraclePair" if a == protocol else "Pair"
        if parent[0] != FACTORY or a == b or b == protocol or (kind == "Pair" and a >= b) or tier not in fees:
            raise ValueError(("market binding", pool))
        if kind not in classes:
            url, record, pair_code = verified(c, pool, kind)
            init = raw(record["creationBytecode"]["onchainBytecode"])
            if not init or factory_code.count(init) != 1:
                raise ValueError(("unbound pair init code", kind))
            classes[kind] = dict(source=url,runtime=digest(pair_code),init=digest(init))
        salt = keccak(raw(a)+raw(b)+tier.to_bytes(2,"big")+raw(protocol))
        address = "0x"+keccak(b"\xff"+raw(FACTORY)+salt+raw(classes[kind]["init"]))[-20:].hex()
        if address != pool or digest(raw(code)) != classes[kind]["runtime"]:
            raise ValueError(("CREATE2 or deployed runtime mismatch",pool))
        lookups.extend((FACTORY,"getPair(address,address,uint16)",["address","address","uint16"],[x,y,tier])
                       for x,y in [(a,b),(b,a)])
        lookups.extend((t,"balanceOf(address)",["address"],[pool]) for t in [a,b,protocol])
        rows.append(dict(pool=pool,kind=kind,tokens=[a,b],fee=tier,reserves=list(map(str,res[:2])),
                         fee_reserve=str(held[0]),oracle=oracle[0]))
    results=iter(posts(c,lookups))
    for row in rows:
        if any(decode(["address"],frame)[0]!=row["pool"] for frame in [next(results),next(results)]):
            raise ValueError(("registry lookup mismatch",row["pool"]))
        row["balances"]=[str(decode(["uint256"],next(results))[0]) for _ in range(3)]
    router_source, _, router_code = verified(c, ROUTER, "Router")
    weth, = c.one(ROUTER,"weth()",returns=["address"])
    oracle, = c.one(ROUTER,"oraclePair()",returns=["address"])
    fee_pair, = c.one(FACTORY,"getPair(address,address,uint16)",["address","address","uint16"],[protocol,weth,10],["address"])
    receipt, = read_batch(endpoint,[("eth_getTransactionReceipt",[ROUTER_RECEIPT])])
    swap_topic=digest(b"Swap(address,uint256,uint256,uint256,uint256,address)")
    if (receipt is None or receipt["status"]!="0x1" or int(receipt["blockNumber"],16)>int(head["number"],16)
        or not any(log["address"].lower() in pools and len(log["topics"])>1 and log["topics"][0]==swap_topic
                   and log["topics"][1][-40:].lower()==ROUTER[2:] for log in receipt["logs"])
        or c.one(ROUTER,"factory()",returns=["address"])!=(FACTORY,)
        or c.one(ROUTER,"protocolToken()",returns=["address"])!=(protocol,) or oracle!=fee_pair or oracle not in pools):
        raise ValueError("deployed router or oracle differs from registered swap")
    balances=posts(c,[(protocol,"balanceOf(address)",["address"],[ROUTER]),(ROUTER,"totalDeposits()",[],[])])
    router=dict(address=ROUTER,runtime=digest(router_code),source=router_source,weth=weth,
                swap_receipt=ROUTER_RECEIPT,oracle_pair=oracle,fee_token_balance=str(int.from_bytes(balances[0])),
                total_deposits=str(int.from_bytes(balances[1])))
    return dict(selector=tag,state_root=ROOT_HASH,height=int(head["number"],16),factory=FACTORY,
                factory_runtime=digest(factory_code),router=router,sources=[REGISTRY_SOURCE,source],protocol_token=protocol,
                fees=fees,classes=classes,markets=rows)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    for endpoint in ENDPOINTS:
        try:
            data=capture(endpoint)
            args.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf-8")
            print(len(data["markets"]), sorted(data["classes"]))
            break
        except (requests.RequestException, ValueError, RuntimeError, KeyError, TypeError) as error:
            print(endpoint,str(error)[:220])
    else:
        raise SystemExit("no independently bound Bundles registry")
