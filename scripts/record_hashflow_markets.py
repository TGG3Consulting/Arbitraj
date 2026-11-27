"""Публичные Hashflow Trade четырёх сетей за один час UTC. Только исходящие запросы для чтения."""
import argparse
import json
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

import requests
from eth_utils import keccak

ROOT = Path(__file__).resolve().parents[1]
DIRECT = ROOT / "tests/data/hashflow_direct_2026-11-26"
COMMIT = "e41cfaadac7d9bacfa368be904a93bd98b5a2df6"
MANIFEST = f"https://raw.githubusercontent.com/hashflownetwork/x-protocol/{COMMIT}/evm/deployed-contracts/IHashflowRouter.json"
TAKER = "https://api.hashflow.com/taker/v3/market-makers"
TRADE = "0x" + keccak(text="Trade(address,address,bytes32,address,address,uint256,uint256)").hex()
# Высоты уже подтверждённых блоков после конца интервала. Значения уточняются по timestamp.
NETWORKS = {
    1: ("ethereum", "mainnet", 0x1946F6A), 56: ("bnb", "bnb", 0x81B3678),
    8453: ("base", "base", 0x3BF57C9), 42161: ("arbitrum", "arbitrum", 0x1F77D2BF),
}
END = int(datetime(2026, 11, 27, 6, tzinfo=timezone.utc).timestamp())


def capture(row, recommended, deployments):
    chain = row["chain_id"]
    name, net, anchor = NETWORKS[chain]
    url = f"https://{net}.gateway.tenderly.co"
    client = requests.Session()

    def rpc(method, params):
        # Обычные вызовы читает тот же публичный узел, что и четырёхсетевая архивная квитанция.
        endpoints = [row["rpc"], url]
        if method == "eth_getLogs":
            endpoints.reverse()
        calls = {"bnb":"bsc", "ethereum":"eth", "base":"base", "arbitrum":"arbitrum"}
        endpoints.append(f"https://{calls[name]}.drpc.org")
        last_error = None
        for endpoint in dict.fromkeys(endpoints):
            try:
                response = client.post(endpoint, json={"jsonrpc":"2.0","id":1,"method":method,"params":params}, timeout=15)
                response.raise_for_status()
                body = response.json()
                if body.get("error"):
                    raise ValueError(body["error"])
                return body["result"]
            except (requests.RequestException, ValueError) as e:
                last_error = e
        raise RuntimeError((chain,method,last_error)) from last_error

    def block(number):
        obj = rpc("eth_getBlockByNumber", [hex(number), False])
        return {"number": number, "hash": obj["hash"], "stateRoot": obj["stateRoot"], "utc": int(obj["timestamp"], 16)}

    def lower_bound(utc):
        low, high = anchor - 500000, anchor
        assert block(low)["utc"] < utc <= block(high)["utc"]
        while low + 1 < high:
            mid = (low + high) // 2
            if block(mid)["utc"] < utc:
                low = mid
            else:
                high = mid
        return high

    assert int(rpc("eth_chainId", []), 16) == chain
    start, stop = lower_bound(END - 3600), lower_bound(END)
    first, last = block(start), block(stop - 1)
    tag = {"blockHash": last["hash"], "requireCanonical": True}
    router = deployments[name]["address"].lower()
    assert router == recommended[str(chain)].lower()

    def call(target, data):
        return rpc("eth_call", [{"to": target, "data": data}, tag])

    def code_hash(target):
        code = bytes.fromhex(rpc("eth_getCode", [target, tag])[2:])
        assert code
        return "0x" + keccak(code).hex()

    def address(word):
        assert len(word) == 66 and int(word[2:26], 16) == 0
        return "0x" + word[-40:].lower()

    logs = []
    step = 1000 if chain in (8453, 56) else stop - start
    for height in range(start, stop, step):
        logs.extend(rpc("eth_getLogs", [{"fromBlock": hex(height), "toBlock": hex(min(height + step, stop) - 1), "topics": [TRADE]}]))
    old_pool = row["trade_logs"][0]["address"].lower()
    roles = {}
    for pool in sorted({old_pool} | {log["address"].lower() for log in logs}):
        allowed = call(router, "0xe816b393" + pool[2:].zfill(64))
        reverse = call(pool, "0xf887ea40")
        roles[pool] = {"authorized": int(allowed, 16) == 1, "pool_router": address(reverse)}
    events = []
    markets = Counter()
    for log in logs:
        pool = log["address"].lower()
        if not roles[pool]["authorized"] or roles[pool]["pool_router"] != router:
            continue
        data = log["data"][2:]
        assert log["topics"] == [TRADE] and len(data) == 64 * 7 and not log.get("removed")
        words = [data[i:i+64] for i in range(0, len(data), 64)]
        asset_in, asset_out = [address("0x" + word) for word in words[3:5]]
        markets[(asset_in, asset_out)] += 1
        events.append({"pool": pool, "tx": log["transactionHash"], "log": int(log["logIndex"], 16),
                       "block": int(log["blockNumber"], 16), "hash": log["blockHash"], "data": log["data"]})
    assert block(stop - 1) == last and block(start) == first
    response = client.get(TAKER, params={"source":"public-evidence", "baseChainType":"evm", "baseChainId":chain}, timeout=15, allow_redirects=False)
    return {"chain":chain, "rpc":url, "first":first, "last":last, "router":router, "runtime":code_hash(router),
            "factory":address(call(router,"0xc45a0155")), "deployment_tx":deployments[name]["deploymentTransactionHash"],
            "unsigned_api":{"url":response.url,"http":response.status_code,"content_type":response.headers.get("Content-Type")},
            "historical_pool":old_pool, "pools":roles, "unfiltered_event_count":len(logs),
            "market_events":[{"in":i,"out":o,"count":n} for (i,o),n in sorted(markets.items())], "events":events}


def main():
    arg = argparse.ArgumentParser(description=__doc__)
    arg.add_argument("output", type=Path)
    out = arg.parse_args().output
    response = requests.get(MANIFEST, timeout=20)
    response.raise_for_status()
    deployments = response.json()
    recommended = json.loads((ROOT / "config/hashflow_routers.json").read_text())["routers"]
    historical = json.loads((DIRECT / "swaps.json").read_text())
    with ThreadPoolExecutor(max_workers=4) as pool:
        snapshots = list(pool.map(lambda row: capture(row, recommended, deployments), historical))
    evidence = {"from_utc":END-3600,"to_utc_exclusive":END,"official_manifest":MANIFEST,"trade_topic":TRADE, "networks":snapshots}
    out.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf8")
    print([(s["chain"],len(s["events"]),len(s["market_events"])) for s in snapshots])


if __name__ == "__main__":
    main()
