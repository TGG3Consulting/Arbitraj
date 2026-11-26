"""Записывает eth_simulateV1 для обычного ArbExecutor.execute на трёх фабриках Slipstream Base.

Только чтение. В изолированном состоянии задаёт код и два конструкторных слота исполнителя,
ETH тестовому публичному адресу. WETH получен deposit(), USDC покупается на отдельной паре
Uniswap V2. Каждый размер/направление стартует от одного канонического блока. Для запуска
нужны requests, eth-abi, eth-utils. Путь к каталогу сохранения передаётся первым аргументом.
"""
import hashlib
import json
import sys
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

ROOT = Path(__file__).resolve().parents[1]
POOL_DATA = ROOT / "tests/data/base_slipstream_2026-11-26/quotes.json"
RUNTIME = ROOT / "contracts/out/ArbExecutor.bin-runtime"
STEP = "(uint8,address,address,address,uint16,bytes)"
OWNER = "0x" + keccak(text="Arbitraj Base 8453 isolated owner 2026-11-26")[-20:].hex()
EXEC = "0x" + keccak(text="Arbitraj Base 8453 isolated executor 2026-11-26")[-20:].hex()


def calldata(sig, types=(), args=()):
    """Сигнатура метода и ABI параметров в hex calldata."""
    return "0x" + (keccak(text=sig)[:4] + encode(types, args)).hex()


def call(to, data, value=0, gas=1_200_000):
    """Вызов от публичного адреса симуляции, без ключа или подписи."""
    return dict(to=to, data=data, value=hex(value), gas=hex(gas), **{"from": OWNER})


def rpc(url, method, params):
    """Только чтение EVM. Возвращает JSON-RPC result или явную ошибку узла."""
    if method not in {"eth_chainId", "eth_getBlockByHash", "eth_getCode", "eth_simulateV1"}:
        raise ValueError("Метод вне перечня чтения")
    response = requests.post(url, json=dict(jsonrpc="2.0", id=1, method=method, params=params),
                             timeout=30)
    response.raise_for_status()
    payload = response.json()
    if "error" in payload:
        raise ValueError(payload["error"])
    return payload["result"]


def make_calls(pool, weth, usdc, router, amount, forward, deadline, minimum):
    """Подготовка входа отдельно от испытуемого пула, квотер, баланс, execute и сверка обеих сторон."""
    source, target = (weth, usdc) if forward else (usdc, weth)
    # Запаса 0,01 ETH хватает на обе измеряемые суммы USDC, торговые пределы бота не меняются.
    funding = amount if forward else 10 ** 16
    calls = [("wrap", call(weth, calldata("deposit()"), funding))]
    approve = lambda spender, value: calldata("approve(address,uint256)", ["address", "uint256"], [spender, value])
    if not forward:
        calls.append(("allow_funding", call(weth, approve(router, funding))))
        data = calldata("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
                        ["uint256", "uint256", "address[]", "address", "uint256"],
                        [funding, amount, [weth, usdc], OWNER, deadline])
        calls.append(("fund_usdc_other_pool", call(router, data)))
    calls.append(("allow_executor", call(source, approve(EXEC, amount))))
    quote = calldata("quoteExactInputSingle((address,address,uint256,int24,uint160))",
                     ["(address,address,uint256,int24,uint160)"], [(source, target, amount, pool["spacing"], 0)])
    calls.append(("quote", call(pool["quoter"], quote)))
    balance = lambda who: calldata("balanceOf(address)", ["address"], [who])
    calls.extend([("owner_in_before", call(source, balance(OWNER))), ("owner_out_before", call(target, balance(OWNER)))])
    data = calldata("execute(address,uint256,address,uint256,uint256," + STEP + "[])",
                    ["address", "uint256", "address", "uint256", "uint256", STEP + "[]"],
                    [source, amount, target, minimum, deadline, [(1, pool["pool"], source, target, 10000, b"")]])
    calls.append(("execute", call(EXEC, data)))
    calls.extend([("owner_in_after", call(source, balance(OWNER))), ("owner_out_after", call(target, balance(OWNER))),
                  ("executor_input", call(source, balance(EXEC))), ("executor_output", call(target, balance(EXEC)))])
    return calls


def main(out_dir):
    """Хранит ответы измерений и адреса. Каждый запрос симуляции использует canonical blockHash."""
    base = json.loads(POOL_DATA.read_text("utf8"))
    profiles = json.loads((ROOT / "config/first_run_dex.json").read_text("utf8"))
    router = next(p["settings"]["dex.router"] for p in profiles if p["id"] == "uniswap-v2-base")
    endpoint, tag = base["endpoint"], base["selector"]
    if int(rpc(endpoint, "eth_chainId", []), 16) != base["chainId"]:
        raise ValueError("Узел другой сети")
    header = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"], False])
    for address in (EXEC, OWNER):
        if rpc(endpoint, "eth_getCode", [address, tag]) != "0x":
            raise ValueError("Тестовый адрес занят контрактом")
    runtime = bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))
    weth, usdc = base["pools"][0]["token0"], base["pools"][0]["token1"]
    word = lambda value: "0x" + format(value, "064x")
    override = {OWNER: {"balance": hex(10 ** 18)}, EXEC: {"code": "0x" + runtime.hex(),
        "stateDiff": {word(0): word(int(OWNER, 16)), word(1): word(int(weth, 16))}}}
    deadline = int(header["timestamp"], 16) + 60
    out = dict(chainId=base["chainId"], block=base["block"], selector=tag, endpoint=endpoint, owner=OWNER,
               executor=EXEC, runtime_sha256=hashlib.sha256(runtime).hexdigest(), router=router,
               weth=weth, usdc=usdc, deadline=deadline, cases=[])
    for i, pool in enumerate(base["pools"]):
        for forward in [True, False]:
            # У первого пула низкая ликвидность. Два других принимают дополнительный размер.
            sizes = [10**12, 10**13] if forward else [10**3, 10**4]
            if i:
                sizes.append(10**16 if forward else 10**7)
            for amount in sizes:
                names = make_calls(pool,weth,usdc,router,amount,forward,deadline,0)
                def run(items):
                    block=dict(stateOverrides=override, calls=[c for _,c in items])
                    answer=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[block]},tag])[0]
                    return {k:r for (k,_),r in zip(items,answer["calls"],strict=True)}
                before = run(names[:next(i+1 for i,(k,_) in enumerate(names) if k=="quote")])
                quote=before["quote"]
                if quote["status"] != "0x1":
                    raise ValueError(quote)
                minimum=int(quote["returnData"][2:66],16)
                calls=make_calls(pool,weth,usdc,router,amount,forward,deadline,minimum)
                results=run(calls)
                case=dict(pool=pool["pool"],quoter=pool["quoter"],forward=forward,amount=str(amount),
                          minimum=str(minimum),calls=dict(calls),results=results)
                out["cases"].append(case)
                print(pool["pool"],forward,amount,results["execute"]["status"],flush=True)
    out_dir.mkdir(exist_ok=True,parents=True)
    (out_dir/"executor.json").write_text(json.dumps(out,indent=2)+"\n",encoding="utf8")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
