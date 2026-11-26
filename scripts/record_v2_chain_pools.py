"""Проверяет размещения V2 на фиксированных блоках, без подписей и отправки.

Вход — подтверждённый Sourcify исходник пары, runtime по хешу блока, каноническая
обратная связь factory.getPair и нетто-комиссия из обеих сторон invariant.
Для воспроизведения передайте путь к источникам и выходному JSON аргументами.
"""
import json
import sys
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, EXEC, RUNTIME, STEP, call, calldata, rpc, simulate_block

ROOT = Path(__file__).resolve().parents[1]
NETWORKS = {
    "base": ("https://base-rpc.publicnode.com", "0x4200000000000000000000000000000000000006"),
    "ethereum": ("https://eth.drpc.org", "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"),
}


def view(url, tag, to, data):
    reply = requests.post(url, json={"jsonrpc": "2.0", "id": 1, "method": "eth_call",
        "params": [{"to": to, "data": data}, tag]}, timeout=20)
    reply.raise_for_status()
    return bytes.fromhex(reply.json()["result"][2:])


def minimum(amount, reserve_in, reserve_out, pips):
    net = amount * (1_000_000 - pips)
    return net * reserve_out // (reserve_in * 1_000_000 + net)


def swap(pool, token_in, token_out, amount, out, deadline, pips):
    args = [token_in, amount, token_out, out, deadline,
            [(0, pool, token_in, token_out, 10000, encode(["uint32"], [pips]))]]
    return call(EXEC, calldata("execute(address,uint256,address,uint256,uint256," + STEP + "[])",
        ["address", "uint256", "address", "uint256", "uint256", STEP + "[]"], args))


def pair_boundary(simulate, pool, weth, other, weth_index, amount, bought, sold, expected_error=None, fund=None):
    """Ask the pair itself for its maximum output, independent of the executor fee input."""
    def transfer(token, qty):
        return call(token, calldata("transfer(address,uint256)", ["address", "uint256"], [pool, qty]))

    def pair_swap(input_index, qty):
        outputs = [0, 0]
        outputs[1 - input_index] = qty
        return call(pool, calldata("swap(uint256,uint256,address,bytes)",
                    ["uint256", "uint256", "address", "bytes"], [*outputs, OWNER, b""]))

    funding = [fund(amount) if fund else call(weth, calldata("deposit()"), amount), transfer(weth, amount)]
    buy = pair_swap(weth_index, bought)
    selling = funding + [buy, transfer(other, bought)]
    results = {}
    for direction, prefix, index, out in (
        ("buy", funding, weth_index, bought), ("sell", selling, 1 - weth_index, sold)
    ):
        for extra in (0, 1):
            calls = simulate(prefix + [pair_swap(index, out + extra)])
            if not all(int(c["status"], 16) == 1 for c in calls[:-1]):
                raise ValueError("pair boundary funding failed: " + direction)
            last = calls[-1]
            status = int(last["status"], 16)
            if status != int(extra == 0):
                raise ValueError("pair maximum differs from formula: " + direction)
            if extra:
                data = last.get("error", {}).get("data") or last.get("returnData", "")
                if expected_error == "empty-revert" and data in ("0x", "") and last.get("error", {}).get("code") == 3:
                    results[direction] = {"max": str(out), "next_revert": "empty-revert"}
                    continue
                if not data.startswith("0x08c379a0"):
                    raise ValueError("missing invariant revert: " + direction)
                reason, = decode(["string"], bytes.fromhex(data[10:]))
                matches = reason == expected_error if expected_error else reason.endswith(": K")
                if not matches:
                    raise ValueError("unexpected pair rejection: " + reason)
                results[direction] = {"max": str(out), "next_revert": reason}
    return results


def measure(row, chain, endpoint=None):
    url, weth = NETWORKS[row["network"]]
    url = endpoint or url
    wrapped = weth
    weth = row.get("funded_asset", wrapped)
    tokens = row["tokens"]
    if weth not in tokens:
        raise ValueError("у выборочной пары нет WETH для измеряемого пополнения")
    tag = {"blockHash": chain["hash"], "requireCanonical": True}
    if int(rpc(url, "eth_chainId", []), 16) != chain["chain"]:
        raise ValueError("chainId не совпал")
    block = rpc(url, "eth_getBlockByHash", [chain["hash"], False])
    if int(block["number"], 16) != chain["block"]:
        raise ValueError("номер блока не совпал")
    pool = row["pool"]
    if keccak(bytes.fromhex(rpc(url, "eth_getCode", [pool, tag])[2:])).hex() != row["runtime_hash"][2:]:
        raise ValueError("байткод пула изменился")
    observed = view(url,tag,row["factory"],calldata("getPair(address,address)",["address","address"],tokens))[-20:]
    if observed.hex() != pool[2:].lower():
        raise ValueError("getPair не вернул исходный пул")
    reserve = view(url,tag,pool,calldata("getReserves()"))
    r = [int.from_bytes(reserve[i:i+32],"big") for i in (0,32)]
    index = tokens.index(weth)
    other = tokens[1-index]
    balance = lambda addr: calldata("balanceOf(address)",["address"],[addr])
    for token, expected in zip(tokens,r):
        if int.from_bytes(view(url,tag,token,balance(pool)),"big") != expected:
            raise ValueError("balanceOf(pool) отличается от резерва")
    denominator, fee = row["fee_fraction"]
    if "fee_getter" in row:
        target = row["fee_target"]
        if target not in (pool, row["factory"]):
            raise ValueError("неизвестный источник комиссии")
        getter = row["fee_getter"]
        if getter == "getFeeForPair(address,address)":
            data=calldata(getter,["address","address"],tokens)
        elif getter == "getFeeForPair(address)":
            data=calldata(getter,["address"],[pool])
        else:
            data=calldata(getter)
        observed_fee, = decode(["uint256"], view(url, tag, target, data))
        if fee != observed_fee:
            raise ValueError("комиссия в ответе контракта не совпала со снимком")
    if fee * 1_000_000 % denominator:
        raise ValueError("комиссия не представима в pips")
    pips = fee * 1_000_000 // denominator
    deadline = int(block["timestamp"],16)+120
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    word=lambda value: "0x"+format(value,"064x")
    overrides={OWNER:{"balance":hex(100*10**18)},EXEC:{"code":"0x"+runtime,"stateDiff":{
        word(0):word(int(OWNER,16)),word(1):word(int(wrapped,16))}}}
    approve=lambda token,amount:call(token,calldata("approve(address,uint256)",["address","uint256"],[EXEC,amount]))
    results=[]
    if weth == wrapped:
        ceilings = [10**15,10**16]
        def fund(qty):
            return call(wrapped,calldata("deposit()"),qty)
    else:
        router = row["funding_router"]
        decimals, = decode(["uint8"], view(url,tag,weth,calldata("decimals()")))
        ceilings = [10**decimals,10*10**decimals]
        def fund(qty):
            payload=calldata("swapETHForExactTokens(uint256,address[],address,uint256)",
                ["uint256","address[]","address","uint256"],[qty,[wrapped,weth],OWNER,deadline])
            return call(router,payload,10**18)
    amounts=sorted(set(min(ceiling, r[index]//depth) for ceiling,depth in zip(ceilings,(10000,500))))
    if len(amounts)!=2 or amounts[0]<=0:raise ValueError("недостаточно резервов для двух размеров")
    for amount in amounts:
        bought=minimum(amount,r[index],r[1-index],pips)
        sold=minimum(bought,r[1-index]-bought,r[index]+amount,pips)
        if not bought or not sold:raise ValueError("округление выхода до нуля")
        prefix=[fund(amount),approve(weth,amount)]
        buy=swap(pool,weth,other,amount,bought,deadline,pips)
        sell=swap(pool,other,weth,bought,sold,deadline,pips)
        mk=lambda inc:swap(pool,weth,other,amount,bought+inc,deadline,pips)
        positive=prefix+[buy,call(other,balance(OWNER)),approve(other,bought),sell,call(weth,balance(OWNER)),call(other,balance(OWNER)),call(other,balance(EXEC)),call(weth,balance(EXEC))]
        def simulate(calls):
            return simulate_block(url,tag,overrides,calls)
        pos=simulate(positive)
        if not all(int(c["status"],16)==1 for c in pos):
            step=next(i for i,c in enumerate(pos) if int(c["status"],16)!=1)
            labels=("fund","approve_buy","buy","buy_balance","approve_sell","sell","sell_balance","wallet_rest","executor_out","executor_in")
            raise ValueError(f"{labels[step]} input={amount}, simulated error {str(pos[step].get('error'))[:140]}")
        if [int(pos[i]["returnData"],16) for i in (3,6,7,8,9)]!=[bought,sold,0,0,0]:
            raise ValueError("фактические балансы не равны формуле V2")
        strict=[simulate(prefix+[mk(1)])[-1],simulate(prefix+[buy,approve(other,bought),swap(pool,other,weth,bought,sold+1,deadline,pips)])[-1]]
        if any(int(c["status"],16)!=0 for c in strict):raise ValueError("minOut + 1 прошёл")
        results.append({"in":str(amount),"bought":str(bought),"returned":str(sold),
            "buy_return":str(int(pos[2]["returnData"],16)),"sell_return":str(int(pos[5]["returnData"],16)),
            "observed_balances":[str(int(pos[i]["returnData"],16)) for i in (3,6,7,8,9)],
            "strict_buy":strict[0].get("error"),"strict_sell":strict[1].get("error")})
        if row.get("verify_pair_boundary"):
            results[-1]["pair_boundary"] = pair_boundary(simulate, pool, weth, other, index, amount, bought, sold, row.get("invariant_error"), fund)
    return dict(row, block_hash=chain["hash"], reserves=[str(v) for v in r], pips=pips, cases=results)


if __name__ == "__main__":
    source,out=map(Path,sys.argv[1:3])
    chains = json.loads((ROOT / "tests/data/base_ethereum_networks.json").read_bytes())
    rows = []
    for source_row in json.loads(source.read_bytes()):
        if not source_row.get("fee_fraction"):
            continue
        # The committed capture can be used as input. Never retain an old
        # success when the fresh block read or either strict control fails.
        measured = {"reason", "block_hash", "reserves", "pips", "cases", "swap_lines"}
        row = {k: v for k, v in source_row.items() if k not in measured}
        try:
            row = measure(row, chains[row["network"]])
            print(row["dex"], "прошло", flush=True)
        except (ValueError, KeyError, requests.RequestException) as error:
            row["reason"] = str(error)[:200]
            print(row["dex"], row["reason"], flush=True)
        rows.append(row)
    out.write_text(json.dumps(rows, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
