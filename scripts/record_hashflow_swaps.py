"""Обновляет публичные истёкшие Hashflow-транзакции по уже отобранным хешам. Только чтение JSON-RPC."""
import json
from pathlib import Path
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
FILE = ROOT / "tests/data/hashflow_direct_2026-11-26/swaps.json"
TRADE = "0x34f57786fb01682fb4eec88d340387ef01a168fe345ea5b76f709d4e560c10eb"
TRANSFER = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"


def read(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    request = Request(url, data=body, headers={"Content-Type": "application/json", "User-Agent": "arbitraj-public-evidence/1"})
    with urlopen(request, timeout=15) as response:
        return json.load(response)["result"]


def capture(row):
    url = row["rpc"]
    hash_ = row["transaction_hash"]
    chain = int(read(url, "eth_chainId", []), 16)
    transaction = read(url, "eth_getTransactionByHash", [hash_])
    receipt = read(url, "eth_getTransactionReceipt", [hash_])
    block = read(url, "eth_getBlockByNumber", [receipt["blockNumber"], False])
    assert chain == row["chain_id"]
    assert transaction["hash"] == receipt["transactionHash"] == hash_
    assert transaction["blockHash"] == receipt["blockHash"] == block["hash"] == row["block_hash"]
    assert transaction["input"] == row["input"] and hash_ in block["transactions"]
    row["nonce"]=int(transaction["nonce"],16)
    topic = row["trade_logs"][0]["topics"][0]
    assert topic == TRADE
    # Дополнительно к событию Trade сохраняются Transfer для независимой проверки реального движения в кошельке.
    row["receipt"] = {key: receipt[key] for key in (
        "status", "blockNumber", "gasUsed", "effectiveGasPrice", "transactionIndex",
        "l1Fee", "operatorFeeScalar", "operatorFeeConstant", "daFootprintGasScalar"
    ) if key in receipt}
    row["receipt"]["logs"] = [log for log in receipt["logs"] if log["topics"] and log["topics"][0] in {topic, TRANSFER}]


if __name__ == "__main__":
    rows = json.loads(FILE.read_text("utf8"))
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=len(rows)) as pool:
        list(pool.map(capture, rows))
    FILE.write_text(json.dumps(rows, indent=2) + "\n", encoding="utf8")
    print("Checked", len(rows), "canonical public receipts")
