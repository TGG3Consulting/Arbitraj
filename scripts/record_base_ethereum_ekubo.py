"""Проверяет пулы Ekubo V3 по событию Core и Router.quote на канонических блоках Base и Ethereum."""
import sys
import time
from types import SimpleNamespace

import requests
from eth_utils import keccak

from record_balancer_deployments import ROOT, read, write
from record_curve_ng_deployments import code

sys.path.insert(0, str(ROOT / "tools/arbitrum_record"))
import ekubo

DOC = "https://docs.ekubo.org/reference/contracts/evm-v3/"
BROADCAST = ("https://raw.githubusercontent.com/EkuboProtocol/evm-contracts/58eef0a/"
             "broadcast/DeployAll.s.sol/8453/run-1785724365212.json")
EVENT = "0x" + keccak(text="PoolInitialized(bytes32,(address,address,bytes32),int32,uint96)").hex()
FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"


class Rpc(ekubo.Rpc):
    """Читает исторические блоки, соблюдая ограничения публичного RPC."""
    def call(self, method, params):
        for delay in [1, 2, 4, 8, 16, None]:
            try:
                return super().call(method, params)
            except (RuntimeError, ekubo.requests.HTTPError) as error:
                if delay is None or not any(x in str(error).lower() for x in ("429", "rate limit", "request limit", "too many")):
                    raise
                time.sleep(delay)


def initialized(rpc, pool_id, block):
    """Находит первый ненулевой sqrtRatio через архивное состояние и проверяет полное событие этого блока."""
    lo, hi = 0, block
    data = ekubo.SEL_SLOAD + pool_id[2:]
    while hi - lo > 1:
        mid = (lo + hi) // 2
        value = rpc.eth_call(ekubo.CORE, data, hex(mid))
        if int(value[2:] or "0", 16):
            hi = mid
        else:
            lo = mid
    criteria = {"address": ekubo.CORE, "fromBlock": hex(hi), "toBlock": hex(hi), "topics": [EVENT]}
    logs = rpc.call("eth_getLogs", [criteria])
    matches = [log for log in logs if log["data"].startswith(pool_id) and not log["removed"]]
    if len(matches) != 1:
        raise ValueError(f"{pool_id}: нет однозначного события инициализации в блоке {hi}")
    event, = matches
    raw = bytes.fromhex(event["data"][2:])
    if len(raw) != 192 or keccak(raw[32:128]) != raw[:32]:
        raise ValueError("PoolId не совпал с ключом события")
    return ("0x" + raw[44:64].hex(), "0x" + raw[76:96].hex(), raw[96:128].hex(), event)


def full_quotes(pool):
    for quote in pool["quotes"]:
        data = bytes.fromhex(quote.get("out", "0x")[2:])
        if len(data) != 64:
            return False
        d0 = int.from_bytes(data[:16], signed=True)
        d1 = int.from_bytes(data[16:32], signed=True)
        amount_in, signed_out = (d1, d0) if quote["is_token1"] else (d0, d1)
        if amount_in != int(quote["amount"]) or signed_out >= 0:
            return False
        config = int(pool["key"]["config"], 16)
        if config & 0x80000000:
            tick = ekubo.signed((int.from_bytes(data[32:]) >> 128) & 0xffffffff, 32)
            word, _ = ekubo.word_index(tick, config & 0x7fffffff)
            if not pool["words"][0] <= word <= pool["words"][1]:
                return False
    return len(pool["quotes"]) == 4


def collect(network):
    e = read(FOLDER / f"{network}.json")
    chain = e["chain_id"]
    rpc = Rpc(e["endpoint"])
    archive = Rpc("https://base.drpc.org") if network == "base" else rpc
    block = rpc.call("eth_getBlockByHash", [e["selector"]["blockHash"], False])
    if int(block["number"], 16) != e["block"] or int(rpc.call("eth_chainId", []), 16) != chain:
        raise ValueError("контрольный блок или сеть расходится")
    addresses = {"dex.ekubo_core": ekubo.CORE.lower(), "dex.ekubo_key_index": ekubo.KEY_INDEX.lower(),
                 "dex.quoter": ekubo.ROUTER.lower()}
    runtime = {key: code(e, addr) for key, addr in addresses.items()}
    url = f"https://prod-api.ekubo.org/poolKeys/{chain}/{ekubo.CORE}?limit=100"
    response = requests.get(url, timeout=20)
    response.raise_for_status()
    index = response.json()
    candidates = [p for p in index["pools"] if int(p["pool_key"]["extension"], 16) == 0
                  and int(p["state"]["liquidity"]) > 0]
    # Нативная пара и пара ERC-20 дают независимые примеры двух форм ключа.
    candidates.sort(key=lambda p: int(p["pool_key"]["token0"], 16) != 0)
    markets = []
    native_forms = set()
    skipped = []
    for candidate in candidates:
        pid = candidate["pool_id"]
        native = int(candidate["pool_key"]["token0"], 16) == 0
        if native in native_forms:
            continue
        key = initialized(archive, pid, e["block"])
        if (int(key[0], 16) == 0) != native or int(key[2][:40], 16) != 0:
            raise ValueError("каталог расходится с ключом ядра")
        args = SimpleNamespace(core=ekubo.CORE, key_index=ekubo.KEY_INDEX, router=ekubo.ROUTER,
                               mev=ekubo.MEV_CAPTURE, words=3, sizes="0.001,0.01", init_keys={pid: key})
        pool = ekubo.record_pool(rpc, pid, args, e["selector"])
        if not full_quotes(pool):
            skipped.append({"pool_id": pid, "quotes": pool["quotes"]})
            revised = []
            for direction in (False, True):
                original = [q for q in pool["quotes"] if q["is_token1"] == direction and "out" in q]
                if len(original) != 2:
                    continue
                offset = 16 if direction else 0
                inputs = [int.from_bytes(bytes.fromhex(q["out"][2:])[offset:offset+16], signed=True)
                          for q in original]
                if all(value == int(q["amount"]) for value, q in zip(inputs, original)):
                    revised.extend(original)
                    continue
                for divisor in (10, 4):
                    amount = max(inputs) // divisor
                    if amount <= 0:
                        continue
                    data = ekubo.quote_data(key[0], key[1], key[2], direction, amount)
                    revised.append(dict(is_token1=direction, amount=str(amount),
                                        out=rpc.eth_call(ekubo.ROUTER, data, e["selector"])))
            pool["quotes"] = revised
            if not full_quotes(pool):
                continue
        native_forms.add(native)
        markets.append(pool)
        if len(native_forms) == 2:
            break
    if len(markets) != 2:
        raise ValueError(f"{network}: найдены {len(markets)} формы ключа, отклонены {skipped}")
    write(FOLDER / f"ekubo_{network}.json", dict(source=DOC, key_index_broadcast=BROADCAST,
          chain_id=chain, selector=e["selector"], endpoint=rpc.url, history_endpoint=archive.url,
          timestamp=int(block["timestamp"], 16),
          catalog=url, catalog_page_count=len(index["pools"]), catalog_has_more=index["has_more"],
          runtime=runtime, pools=markets, incomplete=skipped))
    write(FOLDER / f"{network}.json", e)
    dex = f"ekubo-v3-{network}"
    profile = dict(network=network, chain_id=chain, id=dex, source=DOC,
                   settings={"dex.kind": "ekubo", **addresses})
    return profile, runtime


def main():
    broadcast = requests.get(BROADCAST, timeout=20)
    broadcast.raise_for_status()
    deployments = [tx["contractAddress"].lower() for tx in broadcast.json()["transactions"]
                   if tx.get("contractName") == "PoolKeyIndex"]
    if deployments != [ekubo.KEY_INDEX.lower()]:
        raise ValueError("официальное размещение PoolKeyIndex не совпало")
    base, first = collect("base")
    eth, second = collect("ethereum")
    if first != second:
        raise ValueError("runtime Core, PoolKeyIndex или Router различается между сетями")
    path = ROOT / "config/first_run_dex.json"
    profiles = [p for p in read(path) if (p["network"], p["id"])
                not in {(base["network"], base["id"]), (eth["network"], eth["id"])}]
    write(path, profiles + [base, eth])


if __name__ == "__main__":
    main()
