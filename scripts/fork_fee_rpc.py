"""Архивные RPC для доказательства тарифов фабрик. Чтения состояния привязаны к одному H."""
from itertools import islice
import time

import requests


URLS = {
    "base": ["https://base.gateway.tenderly.co", "https://base.drpc.org",
             "https://base-mainnet.public.blastapi.io", "https://mainnet.base.org", "https://base-rpc.publicnode.com"],
    "ethereum": ["https://rpc.mevblocker.io", "https://eth.drpc.org",
                 "https://rpc.flashbots.net", "https://1rpc.io/eth", "https://ethereum-rpc.publicnode.com"],
}


class Reader:
    def __init__(self, network, head):
        self.session = requests.Session()
        self.endpoints = []
        self.by_method = {}
        self.tag = head["selector"]
        for url in URLS[network]:
            try:
                chain, block = self.request(url, [
                    ("eth_chainId", []), ("eth_getBlockByHash", [self.tag["blockHash"], False])])
                if int(chain, 16) != head["chain_id"] or int(block["number"], 16) != head["block"] or block["hash"] != self.tag["blockHash"]:
                    continue
                self.endpoints.append(url)
                self.header = block
            except (ValueError, KeyError, TypeError, requests.RequestException):
                pass
        if not self.endpoints:
            raise ValueError("no endpoint returned the original chain and head")

    def request(self, url, group):
        batch = [dict(jsonrpc="2.0", id=i, method=m, params=p) for i, (m, p) in enumerate(group)]
        response = self.session.post(url, json=batch[0] if len(batch) == 1 else batch, timeout=16)
        response.raise_for_status()
        body = response.json()
        by_id = {item["id"]: item for item in (body if isinstance(body, list) else [body])}
        if len(by_id) != len(batch) or any(by_id.get(i, {}).get("result") is None for i in range(len(batch))):
            raise ValueError("incomplete "+group[0][0]+": "+str(next((v.get("error") for v in by_id.values() if "error" in v), None))[:100])
        return [by_id[i]["result"] for i in range(len(batch))]

    def fetch(self, group):
        method = group[0][0]
        urls = list(dict.fromkeys([self.by_method.get(method), *self.endpoints]))
        problems = []
        for url in filter(None, urls):
            try:
                result = self.request(url, group)
                self.by_method[method] = url
                return result
            except (ValueError, KeyError, TypeError, requests.RequestException) as exc:
                problems.append(str(exc)[:100])
        if len(group) > 1:
            half=len(group)//2
            return self.fetch(group[:half])+self.fetch(group[half:])
        # Public archive nodes have separate batch, compute-unit and per-second limits.
        for wait in (2,5,10):
            time.sleep(wait)
            for url in filter(None, urls):
                try:
                    result=self.request(url,group)
                    self.by_method[method]=url
                    return result
                except (ValueError,KeyError,TypeError,requests.RequestException) as exc:
                    problems.append(str(exc)[:100])
        raise ValueError((method,problems[-len(urls):]))

    def many(self, ops):
        result = []
        stream = iter(ops)
        while group := list(islice(stream, 6)):
            result.extend(self.fetch(group))
        return result

    def one(self, method, params):
        return self.many([(method, params)])[0]


def pool_blocks(reader, factory, start, finish, initial, final):
    """Смена nonce локализует каждый CREATE даже при молчащем индексаторе."""
    spans = [(start, finish, initial, final)]
    changes = {}
    while spans:
        pending = []
        for lo, hi, left, right in spans:
            if left == right:
                continue
            if left > right:
                raise ValueError("nonmonotonic factory nonce")
            if hi == lo + 1:
                changes[hi] = right - left
            else:
                pending.append((lo, hi, left, right))
        centers = [(lo + hi) // 2 for lo, hi, _, _ in pending]
        answers = reader.many(("eth_getTransactionCount", [factory, hex(h)]) for h in centers)
        spans = []
        for (lo, hi, left, right), mid, value in zip(pending, centers, answers, strict=True):
            n = int(value, 16)
            spans.extend([(lo, mid, left, n), (mid, hi, n, right)])
    return changes
