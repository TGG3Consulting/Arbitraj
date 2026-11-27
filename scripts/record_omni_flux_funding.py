"""Ищет публичных держателей пяти встречных токенов Bin и проверяет код и деньги на архивном хеше."""
import argparse
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
from eth_utils import keccak
from record_base_launch_bindings import read
from record_base_launch_hooks import DATA
from record_base_launch_wallets import PAYER, ZERO
from record_omni_flux_registry import batch, data


def capture():
    with gzip.open(DATA / "omni_flux_registry.json.gz") as stream:
        registry = json.load(stream)
    selector = registry["selector"]
    markets = [row for row in registry["markets"] if row["kind"] == "bin"]
    unknown = sorted({token for row in markets for token in row["key"][:2]} - set(PAYER))

    def lookup(token):
        url = f"https://base.blockscout.com/api/v2/tokens/{token}/holders"
        candidates = []
        try:
            response = requests.get(url, timeout=12)
            response.raise_for_status()
            candidates.extend(item["address"]["hash"].lower() for item in response.json()["items"]
                              if not item["address"].get("is_contract"))
        except (requests.RequestException, ValueError, KeyError) as error:
            reason = str(error)[:180]
        else:
            reason = ""
        creation = next(row["initialize"] for row in markets if token in row["key"][:2])
        transaction = creation["transactionHash"]
        receipt = read("eth_getTransactionReceipt", [transaction])
        if receipt["blockHash"] != creation["blockHash"]:
            raise ValueError("Квитанция Initialize вне закреплённого канона")
        candidates.append(receipt["from"].lower())
        for log in receipt["logs"]:
            if log["address"].lower() == token and len(log["topics"]) == 3 and log["topics"][0] == "0x" + keccak(text="Transfer(address,address,uint256)").hex():
                candidates.extend("0x" + topic[-40:] for topic in log["topics"][1:])
        anchors = []
        newest = registry["requested_end"]
        start = int(creation["blockNumber"], 16)
        transfer = "0x" + keccak(text="Transfer(address,address,uint256)").hex()
        for lo, hi in [(newest - 64, newest), (start, start + 96), (newest - 5000, newest - 65), (start+97,start+5000)]:
            query={"address":token, "topics":[transfer], "fromBlock":hex(lo),"toBlock":hex(hi)}
            try:
                logs=read("eth_getLogs", [query])
            except ValueError:
                if hi-lo<=100:raise
                # Публичные RPC ограничивают число log по частым токенам; короткая выборка остаётся.
                continue
            for log in logs[-60:]:
                if len(log["topics"]) == 3:
                    for topic in log["topics"][1:]:
                        address = "0x" + topic[-40:]
                        if address != ZERO and address not in candidates:
                            candidates.append(address)
                            anchors.append((address, log["transactionHash"]))
        candidates = list(dict.fromkeys(address for address in candidates if address != ZERO))
        calls = [(token, data("balanceOf(address)", ["address"], [address])) for address in candidates]
        answers = [entry for i in range(0,len(calls),40) for entry in batch(selector,calls[i:i+40])]
        ranked = sorted(((int.from_bytes(raw, "big"), address, raw.hex()) for address, (ok, raw) in zip(candidates, answers)
                         if ok and len(raw) == 32 and int.from_bytes(raw, "big")), reverse=True)
        for balance, owner, raw in ranked:
            code = read("eth_getCode", [owner, selector])
            if code == "0x":
                native = read("eth_getBalance", [owner, selector])
                if int(native, 16):
                    return dict(token=token, owner=owner, code=code, balance=str(balance), balance_raw="0x"+raw,
                                balance_calldata="0x"+data("balanceOf(address)", ["address"], [owner]).hex(),
                                native=native, holder_index=url, initialize_tx=transaction, transfer_tx=dict(anchors).get(owner))
        return dict(token=token,reason="Независимый EOA с обоими остатками не найден",holder_index=url,
                    index_error=reason,initialize_tx=transaction,examined=len(candidates))

    with ThreadPoolExecutor(max_workers=3) as pool:
        rows = list(pool.map(lookup, unknown))
    return dict(selector=selector, holders=rows)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(capture(),indent=2,ensure_ascii=False)+"\n",encoding="utf8")
