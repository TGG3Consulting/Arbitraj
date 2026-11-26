"""Read-only archive failover for historically pinned DerpDEX wallet simulations."""
import time
from concurrent.futures import ThreadPoolExecutor
from threading import Lock
import requests

from community_v3_rpc import delay
from record_base_slipstream_executor import simulate_block


URLS = ("https://base-rpc.publicnode.com", "https://mainnet.base.org", "https://base.drpc.org",
        "https://base.gateway.tenderly.co", "https://base.meowrpc.com")


class ArchiveError(RuntimeError):
    def __init__(self, reason, cooldown=0):
        super().__init__(reason)
        self.cooldown = cooldown


def single(url, method, params):
    try:
        response = requests.post(url, json={"jsonrpc":"2.0","id":1,"method":method,"params":params},
                                 timeout=(6, 50))
        if not response.ok:
            raise ArchiveError(f"HTTP {response.status_code}", delay(response.headers.get("Retry-After")))
        body = response.json()
        if body.get("id") != 1 or body.get("result") is None or "error" in body:
            raise ArchiveError(f"{method}: {str(body.get('error', 'missing result'))[:130]}")
        return body["result"]
    except (requests.RequestException, AttributeError, ValueError) as exc:
        raise ArchiveError(str(exc)[:140]) from exc


class Readers:
    def __init__(self, tag, number, root, factory, override, probe, expected, *, chain=8453, urls=URLS, word_reads=True):
        self.tag, self.number, self.root = tag, number, root
        self.word_reads=word_reads
        def verify(url):
            try:
                if int(single(url,"eth_chainId",[]),16) != chain:
                    return None
                self.header(single(url,"eth_getBlockByNumber",[hex(number),False]))
                if not bytes.fromhex(single(url,"eth_getCode",[factory,tag])[2:]):
                    return None
                row, = simulate_block(url, tag, override, [probe], read_rpc=single)
                if row["status"] == "0x1" and row["returnData"] == expected:
                    return url
            except (ArchiveError, RuntimeError, TypeError, ValueError, KeyError):
                pass
            return None
        with ThreadPoolExecutor(max_workers=3) as workers:
            self.urls = list(filter(None, workers.map(verify, urls)))
        if not self.urls:
            raise ArchiveError("no archive reproduced the canonical header and reference override")
        self.lock = Lock()
        self.after = {url:0 for url in self.urls}
        self.failures = {url:0 for url in self.urls}
        self.counts = {url:0 for url in self.urls}

    def header(self, value):
        if (not isinstance(value,dict) or value.get("hash") != self.tag["blockHash"] or
                value.get("stateRoot") != self.root or int(value["number"],16) != self.number):
            raise ArchiveError("archive header disagrees with canonical state")
        return value

    def read(self, _endpoint, method, params):
        if method not in {"eth_call","eth_getBlockByHash","eth_getCode","eth_getTransactionByHash","eth_simulateV1"}:
            raise ValueError("unsupported archive method")
        deadline = time.monotonic()+130
        last = "no provider available"
        while time.monotonic() < deadline:
            with self.lock:
                url = min(self.urls, key=lambda u:self.after[u])
                remaining = self.after[url]-time.monotonic()
            if remaining > 0:
                time.sleep(min(2,remaining))
                continue
            try:
                if method == "eth_simulateV1":
                    group, = params[0]["blockStateCalls"]
                    if params[1] != self.tag:
                        raise ValueError("simulation selector differs")
                    rows = simulate_block(url,self.tag,group["stateOverrides"],group["calls"],read_rpc=single)
                    for row in rows:
                        if row["status"] not in {"0x0","0x1"} or not isinstance(row["returnData"],str):
                            raise ArchiveError("incomplete EVM result")
                        int(row["gasUsed"],16)
                        bytes.fromhex(row["returnData"].removeprefix("0x"))
                    result = [{"parentHash":self.tag["blockHash"],"calls":rows}]
                else:
                    result = single(url,method,params)
                    if method == "eth_getBlockByHash":
                        self.header(result)
                    elif method in {"eth_call","eth_getCode"}:
                        if not isinstance(result,str) or not result.startswith("0x"):
                            raise ArchiveError("invalid hex response")
                        data = bytes.fromhex(result[2:])
                        if self.word_reads and method == "eth_call" and len(data) != 32:
                            raise ArchiveError("unexpected ABI word size")
                    elif result.get("hash") != params[0]:
                        raise ArchiveError("transaction hash mismatch")
                with self.lock:
                    self.counts[url] += 1
                    self.failures[url] = 0
                return result
            except (RuntimeError, KeyError, TypeError, ValueError, AttributeError) as exc:
                last = f"{url} {method}: {exc}"
                with self.lock:
                    self.failures[url] += 1
                    self.after[url] = time.monotonic() + max(getattr(exc,"cooldown",0),
                                                              min(60,2**min(6,self.failures[url])))
        raise ArchiveError("incomplete archive, "+last)
