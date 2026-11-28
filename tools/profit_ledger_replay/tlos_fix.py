"""Проверка квитанции TLOS прямо в прогоне CI (вариант 28.11.2026, вместо correction_map project.py).

Что делает: читает расшифрованную проекцию, берёт квитанцию транзакции 0x2901e120… с публичного узла
BSC и ставит verified_credit двум строкам с флагом tlos — по тем же условиям, что correction_map Codex:
блок 135406118, статус 1, два перевода USDT на кошелёк с logIndex 143 и 144, сумма двух переводов
равна зачисленному в карточке, проверенное зачисление строки = отправлено − 1 USDT.
Кто вызывает: workflow ledger-replay-2.yml после envelope.py open.
Зависит от: requests; адреса узлов — аргументы командной строки (из workflow).
Ошибки: AssertionError — квитанция или карточки не совпали с проверенными; RuntimeError — ни один узел не ответил.
"""
import argparse
import json
from decimal import Decimal
from pathlib import Path

import requests

TX = "0x2901e120419968af20f125154053f1f3532e966eeb5cfa7ed8d14f07fcd1a22f"
WALLET = "0xdfcbe6dd5056c896066db400f414e2c353c095e9"
USDT = "0x55d398326f99059ff775485246999027b3197955"
TRANSFER = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"


def receipt(endpoints):
    """Принимает список узлов BSC; возвращает квитанцию TX. RuntimeError — все узлы отказали."""
    errors = []
    for url in endpoints:
        try:
            r = requests.post(url, json={"jsonrpc": "2.0", "id": 1, "method": "eth_getTransactionReceipt",
                                         "params": [TX]}, timeout=30)
            r.raise_for_status()
            body = r.json()
            if body.get("result"):
                return body["result"]
            errors.append((url, body.get("error")))
        except (requests.RequestException, ValueError) as exc:
            errors.append((url, str(exc)[:200]))
    raise RuntimeError(errors)


def candidates(rec):
    """Принимает квитанцию; возвращает {logIndex: сумма USDT} переводов на кошелёк (как correction_map)."""
    assert rec["transactionHash"] == TX
    assert int(rec["blockNumber"], 16) == 135406118 and int(rec["status"], 16) == 1
    out = {int(log["logIndex"], 16): Decimal(int(log["data"], 16)).scaleb(-18)
           for log in rec["logs"] if log["address"].lower() == USDT and len(log["topics"]) == 3
           and log["topics"][0].lower() == TRANSFER and log["topics"][2][-40:].lower() == WALLET[2:]}
    assert set(out) == {143, 144}
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("projection", type=Path)
    p.add_argument("endpoints", nargs="+")
    args = p.parse_args()
    data = json.loads(args.projection.read_bytes())
    values = candidates(receipt(args.endpoints))
    for name in ("original", "latest"):
        rows = [r for r in data[name] if r["tlos"]]
        assert len(rows) == 2, (name, len(rows))
        chosen = set()
        for row in rows:
            part = row["first"]
            assert part["state"] == "done" and Decimal(part["credited"]) == sum(values.values())
            matches = [v for v in values.values() if v == Decimal(part["sent"]) - 1]
            assert len(matches) == 1
            row["verified_credit"] = str(matches[0])
            chosen.add(matches[0])
        assert len(chosen) == 2
    args.projection.write_text(json.dumps(data, separators=(",", ":")), encoding="utf-8")
    print("TLOS: квитанция проверена, исправлены 2 строки в каждом наборе")


if __name__ == "__main__":
    main()
