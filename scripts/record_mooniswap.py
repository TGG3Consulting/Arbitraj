"""Compare the two official Ethereum Mooniswap factories and their funded native swap paths."""
import argparse
import itertools
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from mooniswap_decay import capture as delayed_quotes
from mooniswap_observe import frame, decode_frame
from fork_fee_rpc import Reader
from record_base_slipstream_executor import OWNER, ROOT, simulate_block
from record_bitswap_ethereum import tx
from record_ethereum_v2_factories import FUNDING, STABLES, WETH
from record_llama_base_cl import CanonicalCalls

ZERO = "0x" + "00" * 20
DAI = "0x6b175474e89094c44da98b954eedeac495271d0f"
MULTI = "0xca11bde05977b3631167028862be2a173976ca11"
FACTORIES = {
    "original": ("0x71cd6666064c3a1354a3b4dca5fa1e2d3ee7d303", "https://github.com/1inch/mooniswap/tree/02dccfab2ddbb8a409400288cb13441763370350"),
    "successor": ("0xbaf9a5d4b0052359326a6cdab54babaa3a3a9643", "https://github.com/1inch/liquidity-protocol/tree/48d3db12262f43139946fb9c3660c418b8b41778"),
}
SWAPPED = {
    "original": "Swapped(address,address,address,uint256,uint256,uint256,uint256,uint256,address)",
    "successor": "Swapped(address,address,address,address,uint256,uint256,uint256,uint256,address)",
}
ASSETS = [ZERO, *STABLES, DAI, WETH]


def aggregate(cap, operations):
    result = []
    for pos in range(0, len(operations), 48):
        group = operations[pos:pos + 48]
        packed = [(to, True, keccak(text=sig)[:4] + encode(types, values)) for to, sig, types, values in group]
        rows, = cap.one(MULTI, "aggregate3((address,bool,bytes)[])", ["(address,bool,bytes)[]"], [packed], ["(bool,bytes)[]"])
        if len(rows) != len(group):
            raise ValueError("incomplete multicall")
        result.extend(raw if ok else None for ok, raw in rows)
    return result


def directory(cap, factory):
    pools, = cap.one(factory, "getAllPools()", returns=["address[]"])
    if not pools or len(pools) != len(set(pools)) or ZERO in pools:
        raise ValueError("factory list")
    tokens = aggregate(cap, [(pool, "getTokens()", [], []) for pool in pools])
    assets = [decode(["address[]"], raw)[0] for raw in tokens]
    if any(len(pair) != 2 or pair[0] >= pair[1] for pair in assets):
        raise ValueError("token identity")
    ops = []
    for pool, (a, b) in zip(pools, assets, strict=True):
        ops.extend([(factory, "isPool(address)", ["address"], [pool]),
                    (factory, "pools(address,address)", ["address"] * 2, [a, b]),
                    (factory, "pools(address,address)", ["address"] * 2, [b, a])])
    checked = iter(aggregate(cap, ops))
    for pool in pools:
        if decode(["bool"], next(checked)) != (True,) or any(decode(["address"], next(checked)) != (pool,) for _ in range(2)):
            raise ValueError("reverse registration " + pool)
    return dict(zip(pools, assets, strict=True))


def inventory(reader, cap, generation, pool, pair):
    code = reader.one("eth_getCode", [pool, reader.tag])
    if code == "0x":
        raise ValueError("absent pool code")
    methods = ["getBalanceForAddition(address)", "getBalanceForRemoval(address)"]
    ops = [(pool, method, ["address"], [asset]) for asset in pair for method in methods]
    getters = ["fee()", "decayPeriod()"] if generation == "original" else ["fee()", "slippageFee()", "decayPeriod()"]
    ops.extend((pool, method, [], []) for method in getters)
    result = aggregate(cap, ops)
    numbers = [str(decode(["uint256"], v)[0]) if v else None for v in result]
    return dict(pool=pool, pair=pair, runtime="0x" + keccak(hexstr=code).hex(), virtual=numbers[:4],
                pricing=dict(zip(getters, numbers[4:], strict=True)))


def read_balance(asset, wallet):
    if asset == ZERO:
        return tx(MULTI, "getEthBalance(address)", ["address"], [wallet])
    return tx(asset, "balanceOf(address)", ["address"], [wallet])


def trials(reader, generation, pool, source, target, amount):
    timestamp = int(reader.header["timestamp"], 16)
    prepare = []
    if source == WETH:
        prepare.append(tx(WETH, "deposit()", value=amount))
    elif source != ZERO:
        prepare.append(tx(FUNDING, "swapETHForExactTokens(uint256,address[],address,uint256)",
                          ["uint256", "address[]", "address", "uint256"], [amount, [WETH, source], OWNER, timestamp + 300], value=10**18))
    if source != ZERO:
        prepare.append(tx(source, "approve(address,uint256)", ["address", "uint256"], [pool, amount]))
    marks = [read_balance(asset, who) for who in (OWNER, pool) for asset in (source, target)]
    quote = tx(pool, "getReturn(address,address,uint256)", ["address", "address", "uint256"], [source, target, amount])

    def simulate(minimum):
        trade = tx(pool, "swap(address,address,uint256,uint256,address)", ["address", "address", "uint256", "uint256", "address"],
                   [source, target, amount, minimum, ZERO], value=amount if source == ZERO else 0)
        observers=frame(pool,[source,target],generation=="successor",MULTI) if minimum==1 else []
        ops = prepare + observers + [quote] + marks + [trade] + marks + observers
        rows = simulate_block(None, reader.tag, {OWNER: {"balance": hex(100 * 10**18)}}, ops,
                              read_rpc=lambda _, method, params: reader.one(method, params))
        if any(row["status"] != "0x1" for row in rows[:len(prepare)]):
            return dict(status="funding_refused")
        head = len(prepare)+len(observers)
        if any(rows[i]["status"] != "0x1" for i in (*range(head+1, head+5), *range(head+6, head+10))):
            return dict(status="balance_unreadable")
        if rows[head]["status"] == "0x1":
            nominal = int(rows[head]["returnData"], 16)
        else:
            nominal = None
        before = [int(item["returnData"], 16) for item in rows[head+1:head+5]]
        after = [int(item["returnData"], 16) for item in rows[head+6:head+10]]
        op = rows[head+5]
        paid = before[0] - after[0]
        received = after[1] - before[1]
        out = dict(status=op["status"], quote=str(nominal) if nominal is not None else None, spent=str(paid),
                   received=str(received), before=list(map(str, before)), after=list(map(str, after)),
                   gas=int(op["gasUsed"], 16), funding_gas=[int(v["gasUsed"], 16) for v in rows[:head]])
        if op["status"] == "0x0":
            if before != after:
                raise ValueError("reverted trade moved assets")
            out["error"] = op.get("error")
            return out
        swapped = "0x"+keccak(text=SWAPPED[generation]).hex()
        events = [log for log in op["logs"] if log["address"].lower() == pool and log["topics"][0] == swapped]
        if len(events) != 1 or paid != amount or received <= 0:
            raise ValueError("unattributed payment")
        if int(op["returnData"], 16) != received or after[2] - before[2] != amount or before[3] - after[3] != received:
            raise ValueError("unexpected wallet or pool cash")
        out.update(logs=op["logs"], trade=trade)
        if observers:
            out["state_before"]=decode_frame(rows[len(prepare):head],generation=="successor")
            out["state_after"]=decode_frame(rows[-len(observers):],generation=="successor")
        return out

    result = dict(input=source, output=target, amount=str(amount))
    probe = simulate(1)
    if probe["status"] != "0x1":
        return dict(result, refusal=probe)
    net = int(probe["received"])
    exact, high = simulate(net), simulate(net + 1)
    if exact["status"] != "0x1" or exact["received"] != str(net) or high["status"] != "0x0":
        raise ValueError("wallet boundary")
    return dict(result, exact=exact, plus_one=high, frame_before=probe["state_before"],frame_after=probe["state_after"])


def activity(reader, registry, generation):
    finish = int(reader.header["number"], 16)
    border = int(reader.header["timestamp"], 16) - 86400
    lo, hi = finish - 8640, finish
    while lo < hi:
        mid = (lo + hi) // 2
        header = reader.one("eth_getBlockByNumber", [hex(mid), False])
        if int(header["timestamp"], 16) < border:
            lo = mid + 1
        else:
            hi = mid
    sig = "0x"+keccak(text=SWAPPED[generation]).hex()
    deals = []
    for first in range(lo, finish+1, 1024):
        logs = reader.one("eth_getLogs", [{"fromBlock":hex(first), "toBlock":hex(min(finish,first+1023)), "topics":[sig]}])
        for log in logs:
            pool = log["address"].lower()
            if pool not in registry:
                continue
            if log.get("removed") or len(log["topics"]) != 4:
                raise ValueError("invalid settlement log")
            decode_address = lambda val: "0x"+val[-40:]
            if generation == "original":
                a, b = map(decode_address, log["topics"][2:4])
                paid, received, *_ = decode(["uint256"]*5+["address"], bytes.fromhex(log["data"][2:]))
            else:
                a = decode_address(log["topics"][3])
                b, paid, received, *_ = decode(["address"]+["uint256"]*4+["address"], bytes.fromhex(log["data"][2:]))
            if {a,b} != set(registry[pool]) or not (paid and received):
                raise ValueError("cash event inconsistent with registered pair")
            deals.append(dict(pool=pool, hash=log["blockHash"], transaction=log["transactionHash"], index=int(log["logIndex"],16),
                              source=a, target=b, input=str(paid), output=str(received)))
    last = reader.one("eth_getBlockByNumber", [hex(finish), False])
    if last["hash"] != reader.tag["blockHash"]:
        raise ValueError("end of log range reorganized")
    return dict(start=lo, end=finish, transactions=deals)


def capture(generation):
    factory, source = FACTORIES[generation]
    head = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    reader = Reader("ethereum", head)
    cap = CanonicalCalls(reader)
    if reader.one("eth_getCode", [OWNER, reader.tag]) != "0x":
        raise ValueError("occupied simulated payer")
    registry = directory(cap, factory)
    targets = [p for p, pair in registry.items() if set(pair).issubset(ASSETS)]
    markets = []
    for p in targets:
        market = inventory(reader, cap, generation, p, registry[p])
        market["trials"] = []
        for a, b in itertools.permutations(registry[p]):
            sizes = (10**6, 10**7) if a in STABLES else (10**18, 10**19) if a == DAI else (10**15, 10**16)
            market["trials"].extend(trials(reader, generation, p, a, b, n) for n in sizes)
        markets.append(market)
    code = reader.one("eth_getCode", [factory, reader.tag])
    tape=activity(reader,registry,generation)
    delay=delayed_quotes(reader,tape["transactions"],targets,generation=="successor",aggregate,MULTI)
    return dict(delay=delay,generation=generation, chain=1, head=head["block"], selector=reader.tag, state_root=reader.header["stateRoot"],
                source=source, factory=factory, activity=tape, runtime="0x"+keccak(hexstr=code).hex(), registry=registry, markets=markets)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("generation", choices=FACTORIES)
    parser.add_argument("output", type=Path)
    options = parser.parse_args()
    options.output.write_text(json.dumps(capture(options.generation), separators=(",", ":")), encoding="utf8")
