"""Публичная диагностика Binance и XT на GitHub. Без аккаунтов и подключения к работающему боту."""
import json
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import UTC, datetime
from decimal import Decimal
from pathlib import Path
from urllib.request import urlopen

from websockets.sync.client import connect


SOURCES = {
    "binance": ("https://data-api.binance.vision/api/v3/depth", "wss://data-stream.binance.vision/stream"),
    "xt": ("https://sapi.xt.com/v4/public/depth", "wss://stream.xt.com/public"),
}


def millis(start):
    return round((time.monotonic() - start) * 1000, 2)


def valid_levels(bids, asks):
    """Положительные обе стороны и пересекающегося стакана нет."""
    bids = [Decimal(p) for p, q in bids if Decimal(p) > 0 and Decimal(q) > 0]
    asks = [Decimal(p) for p, q in asks if Decimal(p) > 0 and Decimal(q) > 0]
    return bool(bids and asks and max(bids) < min(asks))


def depth(venue, coin, start):
    symbol = f"{coin}USDT" if venue == "binance" else f"{coin.lower()}_usdt"
    url = SOURCES[venue][0] + f"?symbol={symbol}&limit=5"
    with urlopen(url, timeout=7) as response:
        book = json.load(response)
    if venue == "xt":
        book = book["result"]
    if not valid_levels(book["bids"], book["asks"]):
        raise ValueError("пустой или пересечённый стакан")
    return {"symbol": symbol, "received_ms": millis(start), "id": str(book["lastUpdateId"])}


def rest_round(venue):
    start = time.monotonic()
    rows = []
    with ThreadPoolExecutor(max_workers=3) as pool:
        requests = {pool.submit(depth, venue, coin, start): coin for coin in ("BTC", "ETH", "USDC")}
        for result in as_completed(requests):
            try:
                rows.append(result.result())
            except Exception as error:
                rows.append({"symbol": requests[result], "error": str(error), "received_ms": millis(start)})
    barrier = max(row["received_ms"] for row in rows)
    for row in rows:
        row["wait_for_old_barrier_ms"] = round(barrier - row["received_ms"], 2)
    return rows


def first_book(venue, previous=None):
    # Открывается только наш клиент. Предыдущая подписка контролируемо закрыта после записи.
    start = time.monotonic()
    stream = "btcusdt@depth5@100ms" if venue == "binance" else "depth@btc_usdt,5"
    method = "SUBSCRIBE" if venue == "binance" else "subscribe"
    with connect(SOURCES[venue][1], open_timeout=6, close_timeout=1) as ws:
        ws.send(json.dumps({"method": method, "params": [stream], "id": "1"}))
        subscribed = time.monotonic()
        while time.monotonic() < start + 12:
            frame = json.loads(ws.recv(timeout=max(0.01, start + 12 - time.monotonic())))
            if venue == "binance" and frame.get("stream") != stream:
                continue
            if venue == "xt" and (frame.get("topic") != "depth" or frame.get("data", {}).get("s") != "btc_usdt"):
                continue
            book = frame["data"]
            if venue == "binance":
                number, bids, asks = book["lastUpdateId"], book["bids"], book["asks"]
            else:
                number, bids, asks = book["i"], book["b"], book["a"]
            if (previous is None or int(number) >= previous) and valid_levels(bids, asks):
                observed = time.monotonic()
                row = {"id": int(number), "subscribe_to_book_ms": millis(subscribed), "open_to_book_ms": millis(start)}
                return row, observed
    raise TimeoutError("свежий двусторонний снимок не получен")


def measure(venue):
    out = {"rest": [], "ws": [], "sources": SOURCES[venue]}
    for _ in range(3):
        out["rest"].append(rest_round(venue))
        time.sleep(1)
    previous = None
    last_at = None
    for _ in range(3):
        try:
            row, observed = first_book(venue, previous)
            if last_at is not None:
                row["previous_frame_to_new_frame_ms"] = round((observed - last_at) * 1000, 2)
            out["ws"].append(row)
            previous = row["id"]
            last_at = observed
        except Exception as error:
            out["ws"].append({"error": str(error)})
            break
    return out


if __name__ == "__main__":
    report = {"utc": datetime.now(UTC).isoformat(), "network": "GitHub Actions public runner"}
    for venue in SOURCES:
        report[venue] = measure(venue)
    Path("public-feed-measurements.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
