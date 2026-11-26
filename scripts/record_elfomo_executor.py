"""Проверяет собранный в GitHub Actions ElfomoExecutor на каноническом блоке Base через eth_simulateV1."""
import sys
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from base_archive_endpoint import available

from record_base_slipstream_executor import EXEC, calldata, rpc
from record_solidly_v3_executor import FOLDER, ORDER, TYPES, read, write


def signature(text):
    return "0x" + keccak(text=text).hex()


def simulate(source, code, owner, entries):
    calls = [{"from": owner, "to": address, "data": data, "gas": hex(1_500_000)}
             for _, address, data in entries]
    override = {EXEC: {"code": "0x" + code}}
    stage, = rpc(source["endpoint"], "eth_simulateV1", [{"blockStateCalls": [
        {"stateOverrides": override, "calls": calls}], "validation": False}, source["selector"]])
    if stage["parentHash"] != source["selector"]["blockHash"]:
        raise ValueError("Сменился канонический блок")
    return calls, dict(zip((name for name, _, _ in entries), stage["calls"], strict=True))


def measure(source, code, case, deadline):
    owner, a, b = case["owner"], case["token_in"], case["token_out"]
    amount, router, vault = int(case["amount"]), source["router"], source["vault"]
    balance = lambda account: calldata("balanceOf(address)", ["address"], [account])
    approval = lambda n: calldata("approve(address,uint256)", ["address", "uint256"], [EXEC, n])
    allowance = lambda payer, spender: calldata("allowance(address,address)", ["address"] * 2, [payer, spender])
    quote = calldata("getAmountOut(address,address,uint256)", ["address", "address", "uint256"], [a, b, amount])
    order = lambda market, out, minimum, expiry: calldata(ORDER, TYPES, [market, a, out, amount, minimum, expiry])
    before = [("wallet_in", a, balance(owner)), ("wallet_out", b, balance(owner)), ("vault_in", a, balance(vault)),
              ("vault_out", b, balance(vault)), ("reset", a, approval(0)), ("approve", a, approval(amount)),
              ("quote", EXEC, quote)]
    _, first = simulate(source, code, owner, before)
    expected = int(first["quote"]["returnData"], 16)
    if first["quote"]["status"] != "0x1" or not expected:
        raise ValueError(f"Нет исполнимой котировки от модуля {a}/{b}/{amount}")
    entries = before + [
        ("wrong_market", EXEC, order(a, b, expected, deadline)), ("wrong_token", EXEC, order(router, router, expected, deadline)),
        ("expired", EXEC, order(router, b, expected, 0)), ("above", EXEC, order(router, b, expected + 1, deadline)),
        ("wallet_allow_after_reject", a, allowance(owner, EXEC)), ("module_allow_after_reject", a, allowance(EXEC, router)),
        ("execute", EXEC, order(router, b, expected, deadline)),
        ("wallet_in_after", a, balance(owner)), ("wallet_out_after", b, balance(owner)),
        ("vault_in_after", a, balance(vault)), ("vault_out_after", b, balance(vault)),
        ("module_in", a, balance(EXEC)), ("module_out", b, balance(EXEC)),
        ("module_allowance", a, allowance(EXEC, router)), ("wallet_allowance", a, allowance(owner, EXEC)),
        ("repeat", EXEC, order(router, b, expected, deadline))]
    calls, results = simulate(source, code, owner, entries)
    rejected = {"wrong_market": "InvalidOrder()", "wrong_token": "UnsupportedToken()", "expired": "InvalidOrder()",
                "above": "InsufficientOutput()", "repeat": "UnsupportedToken()"}
    for key, row in results.items():
        if key in rejected:
            if row["status"] != "0x0" or row["error"].get("data") != signature(rejected[key])[:10]:
                raise ValueError(f"{a}/{b}/{amount}: незаявленный исход отказа {key} {row}")
        elif row["status"] != "0x1":
            raise ValueError(f"{a}/{b}/{amount}: отказ {key} {row.get('error')}")
    n = lambda key: int(results[key]["returnData"], 16)
    if (n("wallet_in") - n("wallet_in_after"), n("wallet_out_after") - n("wallet_out"),
        n("vault_in_after") - n("vault_in"), n("vault_out") - n("vault_out_after"),
        n("quote"), n("execute"), n("wallet_allow_after_reject")) != (amount, expected, amount, expected, expected, expected, amount):
        raise ValueError("Остаток кошелька или vault расходится с фактической сделкой")
    if any(n(key) for key in ("module_allow_after_reject", "module_allowance", "wallet_allowance", "module_in", "module_out")):
        raise ValueError("Остались средства или разрешение")
    topic = signature("ElfomoTrade(uint256,uint256,address,address,address,address,uint256,uint256)")
    trade, = [log for log in results["execute"]["logs"] if log["address"] == router and log["topics"][0] == topic]
    event = decode(["address"] * 4 + ["uint256"] * 2, bytes.fromhex(trade["data"][2:]))
    if int(trade["topics"][2], 16) != 0 or event != (EXEC, owner, a, b, amount, expected):
        raise ValueError("Не совпали плательщик модуля, кошелёк, partnerId или суммы в событии")
    return dict(token_in=a, token_out=b, owner=owner, amount=str(amount), out=str(expected),
                gas=int(results["execute"]["gasUsed"], 16), calls=calls, results=results)


def capture(compiled):
    source = available(read(FOLDER / "elfomofi_base.json"))
    endpoint, tag, runtime = source["endpoint"], source["selector"], compiled["runtime"]
    if int(rpc(endpoint, "eth_chainId", []), 16) != 8453 or rpc(endpoint, "eth_getCode", [EXEC, tag]) != "0x":
        raise ValueError("Неверная сеть или адрес модуля архивного прогона занят")
    header = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"], False])
    deadline = int(header["timestamp"], 16) + 300
    cases = [measure(source, runtime, c, deadline) for c in source["cases"] if c["status"] == "filled"]
    pairs = {(c["token_in"], c["token_out"]) for c in cases}
    if len(pairs) != 4 or any(len({c["amount"] for c in cases if (c["token_in"], c["token_out"]) == p}) != 2 for p in pairs):
        raise ValueError("Неполная матрица двух рынков, направлений и размеров")
    cap = source["cap_probe"]["call"]
    _, exhausted = simulate(source, runtime, cap["from"], [("cap", EXEC, cap["data"])])
    if exhausted["cap"]["status"] != "0x0" or exhausted["cap"]["error"]["data"] != signature("ExhaustedVault()")[:10]:
        raise ValueError("Не подтверждён отказ исчерпания vault")
    return dict(chain_id=8453, selector=tag, endpoint=endpoint, module=EXEC, compiler=compiled["solc"],
                runtime_keccak="0x" + keccak(bytes.fromhex(runtime)).hex(), cases=cases, saturated=exhausted["cap"])


if __name__ == "__main__":
    result = capture(read(Path(sys.argv[1])))
    write(FOLDER / "elfomo_executor_base.json", result)
    print(result["runtime_keccak"], len(result["cases"]), [row["gas"] for row in result["cases"]])
