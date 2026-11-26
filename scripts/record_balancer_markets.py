"""Записывает контрольные котировки Balancer V2/V3 по EIP-1898. Все запросы только eth_call.

V2 выбирает до трёх пулов из снимка исследования, V3 — первые пулы взвешенной фабрики.
Сохраняет исходные вызовы, ответы и ошибки отдельных пулов, допускает только положительные
котировки в обе стороны от установленной фабрики и хранилища. Исполнитель кошелька здесь не вызывается.
"""
import argparse
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write

ZERO = "0x" + "00" * 20


def posts(c, operations):
    output=[]
    multicall = "0xca11bde05977b3631167028862be2a173976ca11"
    for i in range(0,len(operations),64):
        group=operations[i:i+64]
        packed=[(to,True,keccak(text=key)[:4]+encode(args,values)) for to,key,args,values in group]
        results,=c.one(multicall,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[packed],["(bool,bytes)[]"])
        for (to,key,_,_),(ok,value) in zip(group,results,strict=True):
            if not ok:raise ValueError(f"{to} {key} reverted")
            output.append(value)
    return output


class Capture:
    """Запрашивает одну каноническую высоту и сохраняет все результаты, включая откаты."""

    def __init__(self, evidence):
        self.selector = evidence["selector"]
        self.endpoint = evidence["endpoint"]
        self.calls = []
        self.session = requests.Session()

    def batch(self, calls):
        payload = []
        for i, (to, signature, args, values) in enumerate(calls):
            data = keccak(text=signature)[:4] + encode(args, values)
            payload.append(dict(id=i, jsonrpc="2.0", method="eth_call", params=[
                {"from": ZERO, "to": to, "data": "0x" + data.hex()}, self.selector]))
        for attempt in range(3):
            try:
                response = self.session.post(self.endpoint, json=payload, timeout=25)
                response.raise_for_status()
                replies = {r["id"]: r for r in response.json()}
                if set(replies) != set(range(len(calls))):
                    raise ValueError("неполный пакет")
                break
            except (ValueError, requests.RequestException):
                if attempt == 2:
                    raise
        result = []
        for i, c in enumerate(calls):
            r = replies[i]
            self.calls.append(dict(to=c[0], signature=c[1], data=payload[i]["params"][0]["data"],
                                   response=r.get("result") or r.get("error")))
            result.append(bytes.fromhex(r["result"][2:]) if r.get("result") else None)
        return result

    def one(self, to, signature, args=(), values=(), returns=()):
        raw, = self.batch([(to, signature, args, values)])
        if raw is None:
            raise ValueError(f"отказ {signature}")
        return decode(returns, raw)


def samples(network, snapshots, profiles, c):
    v2 = [r for r in profiles if r["settings"]["dex.kind"] == "balancer_v2"]
    v3 = next(r["settings"] for r in profiles if r["settings"]["dex.kind"] == "balancer_v3")
    index = read(snapshots / f"{network}-integration-2026-11-26/sources/sample-pools.json")
    pools = index[v2[0]["id"]]["samples"]
    market = []
    factories = {s[k].lower() for r in v2 for s in [r["settings"]]
                 for k in ("dex.factory", "dex.balancer_stable_factory") if k in s}
    vault = v2[0]["settings"]["dex.vault"].lower()
    for row in pools:
        pool = row["address"].lower()
        ordered = sorted(factories)
        results = []
        for i in range(0, len(ordered), 3):
            results.extend(c.batch([(f, "isPoolFromFactory(address)", ["address"], [pool]) for f in ordered[i:i+3]]))
        owners = [f for f, r in zip(ordered, results) if r and decode(["bool"], r)[0]]
        if len(owners) != 1:
            market.append(dict(version=2, pool=pool, factories=owners, status="factory_unresolved"))
            continue
        pv, = c.one(pool, "getVault()", returns=["address"])
        pid, = c.one(pool, "getPoolId()", returns=["bytes32"])
        registered, _ = c.one(vault, "getPool(bytes32)", ["bytes32"], [pid], ["address", "uint8"])
        if pv != vault or registered != pool:
            raise ValueError(f"{network}: {pool} противоречивое хранилище")
        tokens, balances, _ = c.one(vault, "getPoolTokens(bytes32)", ["bytes32"], [pid], ["address[]","uint256[]","uint256"])
        def quote(i, j, amount):
            deltas, = c.one(vault, "queryBatchSwap(uint8,(bytes32,uint256,uint256,uint256,bytes)[],address[],(address,bool,address,bool))",
                            ["uint8","(bytes32,uint256,uint256,uint256,bytes)[]","address[]","(address,bool,address,bool)"],
                            [0, [(pid, 0, 1, amount, b"")], [tokens[i],tokens[j]], (ZERO, False, ZERO, False)], ["int256[]"])
            if deltas[0] != amount:
                raise ValueError("дельта входа")
            return -deltas[1]
        market.append(observe(2, pool, owners[0], tokens, balances, quote))

    scan, open_pools = scan_pause(c, v3)
    for pool, factory in open_pools:
        claimed, = c.one(factory, "isPoolFromFactory(address)", ["address"], [pool], ["bool"])
        if not claimed:
            raise ValueError(f"фабрика отвергла собственный адрес {pool}")
        tokens, _, balances, _ = c.one(v3["dex.vault"], "getPoolTokenInfo(address)", ["address"], [pool],
                    ["address[]", "(uint8,address,bool)[]", "uint256[]", "uint256[]"])
        def quote(i, j, amount):
            out, = c.one(v3["dex.quoter"], "querySwapSingleTokenExactIn(address,address,address,uint256,address,bytes)",
                         ["address","address","address","uint256","address","bytes"], [pool,tokens[i],tokens[j],amount,ZERO,b""], ["uint256"])
            return out
        market.append(observe(3, pool, factory, tokens, balances, quote))
        if sum(r["version"] == 3 and r["status"] == "quoted" for r in market) == 2:
            break
    return market, scan


def scan_pause(c, config):
    """Читает все зарегистрированные пулы допущенных фабрик. Сохраняет диапазон, ряд и битовую маску паузы."""
    vault = config["dex.vault"]
    all_factories = [a for k, values in config.items() if k.endswith("_factories") for a in values]
    if len(set(all_factories)) != len(all_factories):
        raise ValueError("фабрика в нескольких группах ABI")
    vault_paused, = c.one(vault, "isVaultPaused()", returns=["bool"])
    # Отдельный журнал: ABI групового вызова и упорядоченные аргументы восстановимы из компактного снимка.
    scan_rpc = Capture({"endpoint":c.endpoint, "selector": c.selector})
    aggregation = "0xca11bde05977b3631167028862be2a173976ca11"
    def aggregate(inputs, returns):
        results, = scan_rpc.one(aggregation, "tryAggregate(bool,(address,bytes)[])",
                                ["bool", "(address,bytes)[]"], [True,inputs], ["(bool,bytes)[]"])
        return [decode([returns], data)[0] for ok, data in results if ok]
    counts = aggregate([(f, keccak(text="getPoolCount()")[:4]) for f in all_factories],"uint256")
    open_pools = []
    factories = []
    for factory, count in zip(all_factories, counts, strict=True):
        pools = []
        paused = []
        for start in range(0,count,100):
            args = [start,min(count-start,100)]
            addresses, = scan_rpc.one(factory,"getPoolsInRange(uint256,uint256)",["uint256","uint256"],args,["address[]"])
            if len(addresses) != args[1]:
                raise ValueError(f"{factory} неполное перечисление")
            flags = aggregate([(vault,keccak(text="isPoolPaused(address)")[:4]+encode(["address"],[p])) for p in addresses], "bool")
            for pool, flag in zip(addresses, flags, strict=True):
                if not flag:
                    open_pools.append((pool,factory))
            pools.extend(addresses)
            paused.extend(flags)
        mask = sum(1 << i for i, flag in enumerate(paused) if flag)
        factories.append(dict(factory=factory, pool_count=count, pools=pools,
                              pause_mask_bit_i_for_pool_i=hex(mask), paused=sum(paused)))
    return dict(vault=vault,vault_paused=vault_paused,multicall=aggregation,factories=factories), open_pools


def observe(version, pool, factory, tokens, balances, quote):
    row = dict(version=version, pool=pool, factory=factory, tokens=tokens, balances=list(map(str,balances)), quotes=[])
    try:
        for i, j in [(0, 1), (1, 0)]:
            amount = balances[i] // 100_000
            if amount == 0:
                raise ValueError("недостаточный баланс")
            out = quote(i, j, amount)
            if out <= 0:
                raise ValueError("пустая котировка")
            row["quotes"].append(dict(token_in=tokens[i],token_out=tokens[j],amount_in=str(amount),amount_out=str(out)))
        row["status"] = "quoted"
    except ValueError as err:
        row.update(status="unquoted", reason=str(err))
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshots", type=Path)
    args = parser.parse_args()
    profiles = read(ROOT/"config/first_run_dex.json")
    for network in ("base","ethereum"):
        folder = ROOT/"tests/data/first_run_dex_2026-11-26"
        e = read(folder/f"{network}.json")
        c = Capture(e)
        rows, scan = samples(network, args.snapshots, [r for r in profiles if r["network"]==network], c)
        write(folder/f"balancer_{network}.json", dict(chain_id=e["chain_id"], selector=c.selector, endpoint=c.endpoint,
              sender=ZERO, market=rows, v3_pause_scan=scan, calls=c.calls))
        print(network, [(r["version"],r["status"]) for r in rows], flush=True)


if __name__ == "__main__":
    main()
