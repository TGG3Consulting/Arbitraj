"""Архивные чтения CommunityV3. Проверка узлов, ограничение нагрузки и кэш ответов одного состояния."""
import hashlib
import json
import sqlite3
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime

import requests

BATCH = 3
ENDPOINTS = [
    "https://ethereum-rpc.publicnode.com",
    "https://eth-mainnet.public.blastapi.io",
    "https://mainnet.gateway.tenderly.co",
    "https://1rpc.io/eth",
    "https://eth.drpc.org",
    "https://eth.meowrpc.com",
]


class Unavailable(Exception):
    def __init__(self, message, retry_after=0):
        super().__init__(message)
        self.retry_after = retry_after


def delay(value):
    if not value:
        return 0
    try:
        return max(0, float(value))
    except ValueError:
        try:
            date = parsedate_to_datetime(value)
            return max(0, (date - datetime.now(timezone.utc)).total_seconds())
        except (ValueError, TypeError, OverflowError):
            return 0


class Node:
    def __init__(self, url):
        self.url = url
        self.http = requests.Session()
        self.available = 0
        self.failures = 0
        self.requests = 0

    def read(self, calls):
        payload = [dict(jsonrpc="2.0", id=i, method=method, params=params)
                   for i, (method, params) in enumerate(calls)]
        try:
            response = self.http.post(self.url, json=payload, timeout=(6, 50))
            if not response.ok:
                raise Unavailable(f"HTTP {response.status_code}", delay(response.headers.get("Retry-After")))
            raw = response.json()
            if not isinstance(raw, list):
                raise Unavailable("batch request was rejected")
            by_id = {entry["id"]: entry for entry in raw}
            if len(raw) != len(calls) or set(by_id) != set(range(len(calls))):
                raise Unavailable("incomplete JSON-RPC response")
            rows = []
            for i in range(len(calls)):
                reply = by_id[i]
                if "error" in reply:
                    msg = str(reply["error"].get("message", "")).lower()
                    # A provider quota or RPC gas cap says nothing about contract execution.
                    if "execution reverted" not in msg and msg.strip() != "out of gas":
                        raise Unavailable(msg[:150])
                    rows.append({"error": reply["error"]})
                elif "result" in reply:
                    rows.append({"result": reply["result"]})
                else:
                    raise Unavailable("missing result")
            self.requests += 1
            return rows
        except (requests.RequestException, KeyError, ValueError, TypeError) as exc:
            raise Unavailable(str(exc)[:160]) from exc

    def verify(self, selector, height, state_root, probe, expected):
        hash_ = selector["blockHash"]
        result = self.read([("eth_chainId", []), ("eth_getBlockByNumber", [hex(height), False]), probe])
        hdr = result[1].get("result") or {}
        if (result[0].get("result") != "0x1" or hdr.get("hash") != hash_
                or hdr.get("stateRoot") != state_root or result[2].get("result") != expected):
            raise Unavailable("canonical header or reference state override mismatch")
        return self


class Readers:
    def __init__(self, cache_file, selector, height, state_root, probe, expected):
        self.pool = ThreadPoolExecutor(max_workers=3)
        self.cache = sqlite3.connect(cache_file)
        self.cache.execute("CREATE TABLE IF NOT EXISTS replies(key TEXT PRIMARY KEY, node TEXT NOT NULL, data TEXT NOT NULL)")
        self.nodes = []
        self.cached = 0
        def verify(url):
            node = Node(url)
            try:
                return node.verify(selector, height, state_root, probe, expected)
            except Unavailable:
                node.http.close()
                return None
        for node in self.pool.map(verify, ENDPOINTS):
            if node:
                self.nodes.append(node)
        if not self.nodes:
            raise RuntimeError("no archive endpoint reproduced the canonical header and reference quote")
        self.cursor = 0

    def __enter__(self):
        return self

    def __exit__(self, *unused):
        self.pool.shutdown()
        for node in self.nodes:
            node.http.close()
        self.cache.close()

    def batch(self, calls):
        replies = [None] * len(calls)
        pending = []
        for i, call in enumerate(calls):
            key = hashlib.sha256(json.dumps(call, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
            row = self.cache.execute("SELECT data FROM replies WHERE key=?", (key,)).fetchone()
            if row:
                replies[i] = json.loads(row[0])
                self.cached += 1
            else:
                pending.append((i, key, call))
        groups = [pending[i:i+BATCH] for i in range(0, len(pending), BATCH)]
        last_progress = time.monotonic()
        while groups:
            now = time.monotonic()
            if now - last_progress > 360:
                raise RuntimeError(f"archive providers stalled, {sum(map(len,groups))} calls remain")
            nodes = self.nodes[self.cursor:] + self.nodes[:self.cursor]
            self.cursor = (self.cursor + 1) % len(self.nodes)
            available = [node for node in nodes if node.available <= now][:min(3, len(groups))]
            if not available:
                time.sleep(min(4, max(.1, min(n.available for n in nodes) - now)))
                continue
            work = [(node, group, self.pool.submit(node.read, [v[2] for v in group]))
                    for node, group in zip(available, groups)]
            groups = groups[len(work):]
            for node, group, future in work:
                try:
                    result = future.result()
                except Unavailable as exc:
                    node.failures += 1
                    node.available = time.monotonic() + max(exc.retry_after, min(90, 2**min(7, node.failures)))
                    groups.append(group)
                    continue
                node.failures = 0
                node.available = time.monotonic() + .75
                for (i, key, _), answer in zip(group, result, strict=True):
                    replies[i] = answer
                    self.cache.execute("INSERT OR REPLACE INTO replies VALUES (?,?,?)",
                                       (key, node.url, json.dumps(answer)))
                self.cache.commit()
                last_progress = time.monotonic()
        return replies
