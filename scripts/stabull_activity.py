"""Публичный журнал Trade семи Ethereum Curve. Последний заголовок каждой страницы задаёт следующее окно."""
import json
import time


PORTAL = "https://portal.sqd.dev/datasets/ethereum-mainnet/finalized-stream"


def trades(session, first, last, head_hash, pools, topic):
    query = {
        "type": "evm", "toBlock": last,
        "fields": {"block": {"number": True, "hash": True},
                   "log": {"address": True, "logIndex": True, "transactionHash": True,
                           "topics": True, "data": True}},
        "logs": [{"address": pools, "topic0": [topic]}],
    }
    cursor = first
    logs, pages = [], []
    while cursor <= last:
        for attempt in range(4):
            response = session.post(PORTAL, json={**query, "fromBlock": cursor}, timeout=25)
            if response.status_code not in (429, 529):
                break
            retry = response.headers.get("Retry-After", "")
            if not retry.isdigit() or int(retry) > 60 or attempt == 3:
                response.raise_for_status()
            time.sleep(max(1, int(retry)))
        response.raise_for_status()
        blocks = [json.loads(line) for line in response.iter_lines() if line]
        heights = [b["header"]["number"] for b in blocks]
        if not heights or heights != sorted(set(heights)) or not cursor <= heights[0] <= heights[-1] <= last:
            raise ValueError("Неполный ответ публичного журнала Stabull")
        for block in blocks:
            header = block["header"]
            for log in block.get("logs", []):
                if log["address"].lower() not in pools or log["topics"][0] != topic:
                    raise ValueError("В журнале другой источник Trade")
                logs.append({**log, "blockNumber": hex(header["number"]), "blockHash": header["hash"],
                             "logIndex": hex(log["logIndex"])})
        end = blocks[-1]["header"]
        pages.append({"from": cursor, "through": end["number"], "hash": end["hash"]})
        cursor = end["number"] + 1
    if pages[-1]["hash"] != head_hash:
        raise ValueError("Индекс и исходный блок Stabull расходятся")
    return logs, pages

