"""Архивный Ethereum Ring. Только eth_call, eth_getCode и изолированные eth_simulateV1.

Вход профиля — реальные underlying, а путь в роутере — Few wrappers. Проверяет оба
направления и обе границы (номинальная котировка и баланс получателя), обе формы ETH.
"""
import json
import sys
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, call, calldata, rpc
from record_v2_chain_pools import minimum, view

ROOT = Path(__file__).resolve().parents[1]
FACTORY = "0xeb2a625b704d73e82946d8d026e1f588eed06416"
FEW = "0x7d86394139bf1122e82fdf45bb4e3b038a4464dd"
QUOTER = "0x39d1d8fcc5e6eeaf567bce4e29b94fec956d3519"
ROUTER = "0xeff87a51f5abd015f1afcd5737bbab450ea15a24"
PERMIT = "0x000000000022d473030f116ddee9f6b43ac78ba3"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
ASSETS = [WETH, "0xdac17f958d2ee523a2206206994597c13d831ec7", "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
          "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599", "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984"]
ADDRESS_THIS = "0x" + "00" * 19 + "02"
BALANCE = "balanceOf(address)"


def get_address(raw):
    return decode(["address"], raw)[0]


def allow(token, amount):
    return [
        call(token, calldata("approve(address,uint256)", ["address", "uint256"], [PERMIT, amount])),
        call(PERMIT, calldata("approve(address,address,uint160,uint48)",
                              ["address", "address", "uint160", "uint48"], [token, ROUTER, amount, 2**48-1])),
    ]


def exchange(path, out_asset, amount, bound, deadline, native_input=False, native_output=False, check=None, legacy=False):
    receiver = ADDRESS_THIS if native_output else OWNER
    commands = [0x23]
    inputs = [encode(["address", "uint256", "uint256", "address[]", "bool", "bool"],
                     [receiver, amount, bound, path, not native_input, True])]
    if path[0] == "0xef87f4608e601e8564800265aee1c1ffadf73283" and not legacy:
        pool = "0x147d15e009a63ebed5196ea029679329204f98fd"
        sub = [
            encode(["address", "address", "uint160"], [ASSETS[1], ADDRESS_THIS, amount]),
            encode(["address", "address", "uint256"], [ASSETS[1], pool, amount]),
            encode(["address", "uint256", "uint256", "address[]", "bool", "bool"], [receiver, 0, bound, path, False, True]),
        ]
        commands = [0x21]
        inputs = [encode(["bytes", "bytes[]"], [bytes([0x02, 0x27, 0x23]), sub])]
    if native_input:
        commands.insert(0, 0x0b)
        inputs.insert(0, encode(["address", "uint256"], [ADDRESS_THIS, amount]))
    if native_output:
        commands.append(0x0c)
        inputs.append(encode(["address", "uint256"], [OWNER, bound]))
    else:
        commands.append(0x0e)
        inputs.append(encode(["address", "address", "uint256"], [OWNER, out_asset, bound if check is None else check]))
    return call(ROUTER, calldata("execute(bytes,bytes[],uint256)", ["bytes", "bytes[]", "uint256"],
                                 [bytes(commands), inputs, deadline]), amount if native_input else 0)


def measure(endpoint, tag, deadline, asset, wrappers, reserves, amount, native):
    path = [wrappers[WETH], wrappers[asset]]
    quoted = minimum(amount, *reserves, 3000)
    returned = minimum(quoted, reserves[1] - quoted, reserves[0] + amount, 3000)
    if min(quoted, returned) <= 0:
        raise ValueError("нулевое округление")
    labels, calls, fails = [], [], set()

    def add(label, tx, fail=False):
        labels.append(label)
        calls.append(tx)
        if fail:
            fails.add(label)

    def balance(label, token, who):
        add(label, call(token, calldata(BALANCE, ["address"], [who])))

    def balances(prefix):
        for label, token, who in [("owner_weth", WETH, OWNER), ("owner_other", asset, OWNER),
                                  ("router_weth", WETH, ROUTER), ("router_other", asset, ROUTER),
                                  ("router_fw0", path[0], ROUTER), ("router_fw1", path[1], ROUTER),
                                  ("owner_fw0", path[0], OWNER), ("owner_fw1", path[1], OWNER)]:
            balance(prefix + label, token, who)
    if not native:
        add("weth_deposit", call(WETH, calldata("deposit()"), amount))
        for i, tx in enumerate(allow(WETH, amount)):
            add("weth_approval" + str(i), tx)
    balances("before_")
    for direction, (p, input_size, output_min, token_out) in enumerate([
            (path, amount, quoted, asset), (list(reversed(path)), quoted, returned, WETH)]):
        buy = direction == 0
        if not buy:
            for i, tx in enumerate(allow(asset, quoted)):
                add("other_approval" + str(i), tx)
        add("quote" + str(direction), call(QUOTER, calldata("getAmountsOut(uint256,address[])",
                                                           ["uint256", "address[]"], [input_size, p])))
        def swap(bound, check=None):
            return exchange(p, token_out, input_size, bound, deadline, native and buy, native and not buy, check)
        if not buy and asset == ASSETS[1]:
            add("legacy_usdt_nonzero_approve", exchange(p, token_out, input_size, output_min, deadline,
                native_output=native, legacy=True), True)
        add("nominal_plus_one" + str(direction), swap(output_min + 1), True)
        if buy or not native:
            add("balance_plus_one" + str(direction), swap(output_min, output_min + 1), True)
        add("swap" + str(direction), swap(output_min))
        balances("bought_" if buy else "sold_")
    overrides = {OWNER: {"balance": hex(10**19)}}
    frame = rpc(endpoint, "eth_simulateV1", [{"blockStateCalls": [{"stateOverrides": overrides, "calls": calls}]}, tag])[0]
    got = dict(zip(labels, frame["calls"], strict=True))
    for label, row in got.items():
        if int(row["status"], 16) != int(label not in fails):
            raise ValueError(f"{asset} {amount} native={native} {label}: unexpected result {row.get('error')}")
    if asset == ASSETS[1]:
        error = got["legacy_usdt_nonzero_approve"]["error"].get("data", "")
        if not error.startswith("0x08c379a0") or decode(["string"], bytes.fromhex(error[10:]))[0] != "APPROVE_FAILED":
            raise ValueError("не подтверждена причина отказа прежнего пути USDT")
    def value(label):
        return int(got[label]["returnData"], 16)
    for d, expected in enumerate([[amount, quoted], [quoted, returned]]):
        if list(decode(["uint256[]"], bytes.fromhex(got["quote"+str(d)]["returnData"][2:]))[0]) != expected:
            raise ValueError("расхождение модели 997/1000 и Quoter")
    for key in ["router_weth", "router_other", "router_fw0", "router_fw1", "owner_fw0", "owner_fw1"]:
        if not value("before_"+key) == value("bought_"+key) == value("sold_"+key):
            raise ValueError("изменился посторонний остаток: " + key)
    observations = [[value(p+"owner_weth"), value(p+"owner_other")] for p in ["before_", "bought_", "sold_"]]
    if observations != [[0 if native else amount, 0], [0, quoted], [0 if native else returned, 0]]:
        raise ValueError("остатки плательщика по ERC20 расходятся с котировкой")
    native_transfers = []
    if native:
        # WETH9 списывает msg.sender с Withdrawal и перечисляет ETH роутеру; после UNWRAP
        # отдельно проверяем фактическую выплату ETH кошельку через traceTransfers.
        trace = rpc(endpoint, "eth_simulateV1", [{"traceTransfers": True,
            "blockStateCalls": [{"stateOverrides": overrides, "calls": calls}]}, tag])[0]["calls"]
        rows = trace[labels.index("swap1")]["logs"]
        transfers = [log for log in rows if log["address"].lower() == "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                     and len(log["topics"]) == 3 and log["topics"][1][-40:].lower() == ROUTER[2:]
                     and log["topics"][2][-40:].lower() == OWNER[2:]]
        native_transfers = transfers
        if sum(int(log["data"], 16) for log in transfers) != returned:
            raise ValueError("отсутствует точная выплата ETH получателю")
    return {"in": str(amount), "out": str(quoted), "roundtrip": str(returned), "native": native,
            "balances": [[str(x) for x in row] for row in observations], "native_transfers": native_transfers,
            "swaps": [{"direction": d, "target": ROUTER, "calldata": calls[labels.index("swap"+str(d))]["data"],
                       "value": calls[labels.index("swap"+str(d))]["value"],
                       "logs": got["swap"+str(d)]["logs"]} for d in [0, 1]], "calls": [
                {"label": k, "status": v["status"], "gasUsed": v["gasUsed"], "returnData": v.get("returnData"),
                 **({"error": v["error"]} if "error" in v else {})} for k, v in got.items()]}


def main(output):
    snapshot = json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_text())
    endpoint, tag = snapshot["endpoint"], snapshot["selector"]
    if int(rpc(endpoint, "eth_chainId", []), 16) != 1:
        raise ValueError("чужая сеть")
    header = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"], False])
    if header["hash"] != tag["blockHash"] or int(header["number"], 16) != snapshot["block"]:
        raise ValueError("заголовок блока не совпал")
    def read(addr, sig, types=(), args=()):
        return view(endpoint, tag, addr, calldata(sig, types, args))
    wrappers, decimals, codes, pools = {}, {}, {}, []
    def code(addr):
        buf = bytes.fromhex(rpc(endpoint, "eth_getCode", [addr, tag])[2:])
        if not buf:
            raise ValueError("пустой runtime")
        codes[addr] = {"bytes": len(buf), "keccak256": "0x"+keccak(buf).hex()}
    for addr in [ROUTER, QUOTER, FACTORY, FEW, PERMIT]:
        code(addr)
    for asset in ASSETS:
        fw = get_address(read(FEW, "getWrappedToken(address)", ["address"], [asset]))
        if get_address(read(fw, "token()")) != asset or get_address(read(fw, "factory()")) != FEW:
            raise ValueError("обратная связь wrapper")
        prec = int.from_bytes(read(asset, "decimals()"), "big")
        if int.from_bytes(read(fw, "decimals()"), "big") != prec:
            raise ValueError("decimals отличаются")
        wrappers[asset], decimals[asset] = fw, prec
        code(fw)
    for asset in ASSETS[1:]:
        path = [wrappers[WETH], wrappers[asset]]
        pool = get_address(read(FACTORY, "getPair(address,address)", ["address"]*2, path))
        code(pool)
        order = [get_address(read(pool, sig)) for sig in ("token0()", "token1()")]
        if order != sorted(path):
            raise ValueError("стороны пары не совпали")
        reserves = decode(["uint112", "uint112", "uint32"], read(pool, "getReserves()"))[:2]
        ri, ro = reserves if order[0] == path[0] else reversed(reserves)
        sizes = [10**15, 10**16]
        cases = [measure(endpoint, tag, int(header["timestamp"], 16)+120, asset, wrappers,
                         (ri, ro), size, native) for native in [False, True] for size in sizes]
        pools.append({"asset": asset, "pool": pool, "reserve_weth": str(ri), "reserve_other": str(ro),
                      "cases": cases})
    Path(output).write_text(json.dumps({"chain_id": 1, "block": snapshot["block"], "selector": tag,
        "addresses": {"quoter": QUOTER, "router": ROUTER, "factory": FACTORY, "few": FEW, "permit2": PERMIT},
        "codes": codes, "wrappers": wrappers, "decimals": decimals, "pools": pools}, indent=2)+"\n")


if __name__ == "__main__":
    main(sys.argv[1])
