"""Проверяет размещения V2 на фиксированных блоках, без подписей и отправки.

Вход — подтверждённый Sourcify исходник пары, runtime по хешу блока, каноническая
обратная связь factory.getPair и нетто-комиссия из обеих сторон invariant.
Для воспроизведения передайте путь к источникам и выходному JSON аргументами.
"""
import json
import sys
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, EXEC, RUNTIME, STEP, call, calldata, rpc

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


def measure(row, chain):
    url, weth = NETWORKS[row["network"]]
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
    if fee * 1_000_000 % denominator:
        raise ValueError("комиссия не представима в pips")
    pips = fee * 1_000_000 // denominator
    deadline = int(block["timestamp"],16)+120
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    word=lambda value: "0x"+format(value,"064x")
    overrides={OWNER:{"balance":hex(100*10**18)},EXEC:{"code":"0x"+runtime,"stateDiff":{
        word(0):word(int(OWNER,16)),word(1):word(int(weth,16))}}}
    approve=lambda token,amount:call(token,calldata("approve(address,uint256)",["address","uint256"],[EXEC,amount]))
    results=[]
    amounts=sorted(set(min(ceiling, r[index]//depth) for ceiling,depth in [(10**15,10000),(10**16,500)]))
    if len(amounts)!=2 or amounts[0]<=0:raise ValueError("недостаточно резервов для двух размеров")
    for amount in amounts:
        bought=minimum(amount,r[index],r[1-index],pips)
        sold=minimum(bought,r[1-index]-bought,r[index]+amount,pips)
        if not bought or not sold:raise ValueError("округление выхода до нуля")
        prefix=[call(weth,calldata("deposit()"),amount),approve(weth,amount)]
        buy=swap(pool,weth,other,amount,bought,deadline,pips)
        sell=swap(pool,other,weth,bought,sold,deadline,pips)
        mk=lambda inc:swap(pool,weth,other,amount,bought+inc,deadline,pips)
        positive=prefix+[buy,call(other,balance(OWNER)),approve(other,bought),sell,call(weth,balance(OWNER)),call(other,balance(OWNER)),call(other,balance(EXEC)),call(weth,balance(EXEC))]
        def simulate(calls):
            return rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":overrides,"calls":calls}]},tag])[0]["calls"]
        pos=simulate(positive)
        if not all(int(c["status"],16)==1 for c in pos):
            raise ValueError("не прошёл полный круг: "+str(next(c.get("error") for c in pos if int(c["status"],16)!=1))[:100])
        if [int(pos[i]["returnData"],16) for i in (3,6,7,8,9)]!=[bought,sold,0,0,0]:
            raise ValueError("фактические балансы не равны формуле V2")
        strict=[simulate(prefix+[mk(1)])[-1],simulate(prefix+[buy,approve(other,bought),swap(pool,other,weth,bought,sold+1,deadline,pips)])[-1]]
        if any(int(c["status"],16)!=0 for c in strict):raise ValueError("minOut + 1 прошёл")
        results.append({"in":str(amount),"bought":str(bought),"returned":str(sold),
            "buy_return":str(int(pos[2]["returnData"],16)),"sell_return":str(int(pos[5]["returnData"],16)),
            "observed_balances":[str(int(pos[i]["returnData"],16)) for i in (3,6,7,8,9)],
            "strict_buy":strict[0].get("error"),"strict_sell":strict[1].get("error")})
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
