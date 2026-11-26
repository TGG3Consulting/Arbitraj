"""За контрольные 24 часа считает одну фактическую ногу USDC по журналу Fill подтверждённого Entry."""
import hashlib
import json
from collections import defaultdict

import requests
from eth_abi import decode
from eth_utils import keccak

from record_baibai import ENTRY, FILL, FOLDER, USDC, WETH, read, write


def capture():
    w = read(FOLDER / "elfomo_volume_base.json")
    base = read(FOLDER / "base.json")
    if base["selector"]["blockHash"] != w["end"]["hash"]:
        raise ValueError("Окно не на контрольном блоке")
    session = requests.Session()
    def rpc(method, params):
        if method not in ("eth_getBlockByNumber", "eth_getLogs"):
            raise ValueError("Метод вне перечня чтения")
        response = session.post(base["endpoint"], json=dict(id=1, jsonrpc="2.0", method=method, params=params), timeout=35)
        response.raise_for_status()
        return response.json()["result"]
    def verify(bound):
        h = w[bound]
        r = rpc("eth_getBlockByNumber", [hex(h["number"]), False])
        if r["hash"] != h["hash"] or int(r["timestamp"], 16) != h["timestamp"]:
            raise ValueError("Сменилась каноническая граница окна")
    for bound in ("previous", "start", "end"):
        verify(bound)
    modern, legacy = ["0x" + keccak(text=s).hex() for s in (FILL, "Fill(address,uint64,address,address,uint256,uint256)")]
    ids, pages = set(), []
    rows = defaultdict(lambda: dict(count=0, usdc_raw=0))
    for first in range(w["start"]["number"], w["end"]["number"] + 1, 6000):
        last = min(w["end"]["number"], first + 5999)
        logs = rpc("eth_getLogs", [dict(address=ENTRY, fromBlock=hex(first), toBlock=hex(last), topics=[[modern, legacy]])])
        pages.append(dict(first=first, last=last, count=len(logs), sha256=hashlib.sha256(
            json.dumps(logs, sort_keys=True, separators=(",", ":")).encode()).hexdigest()))
        for log in logs:
            key = log["transactionHash"], log["logIndex"]
            if (key in ids or log["address"] != ENTRY or log.get("removed")
                    or not first <= int(log["blockNumber"], 16) <= last or len(log["topics"]) != 4):
                raise ValueError("Чужое, повторное или заменённое событие")
            event = log["topics"][0]
            if event not in (modern, legacy):
                raise ValueError("Неизвестная версия события")
            ids.add(key)
            coin = "0x" + log["topics"][1][-40:]
            token, dx, dy, *_ = decode(["address"] + ["uint256"] * (3 if event == modern else 2),
                                        bytes.fromhex(log["data"][2:]))
            if token not in (coin, USDC) or coin == USDC:
                raise ValueError("У события нет однозначной ноги USDC")
            row = rows[(coin, token, event)]
            row["count"] += 1
            row["usdc_raw"] += dx if token == USDC else dy
            row["example_tx"] = key[0]
    verify("end")
    return dict(chain_id=8453, entry=ENTRY, primary_pair=[WETH, USDC], start=w["start"], end=w["end"],
                count=len(ids), pages=pages, markets=[dict(base=a, token_in=b, event=kind, **row)
                                                     for (a, b, kind), row in sorted(rows.items())])


if __name__ == "__main__":
    write(FOLDER / "baibai_volume_base.json", capture())
