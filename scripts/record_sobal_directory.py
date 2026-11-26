"""Enumerate Sobal Base Vault registrations by monotone pool nonce and canonical creation receipts."""
import argparse
import json
from pathlib import Path
from time import sleep

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT

VAULT = "0x7122e35cec2eed4a989d9b0a71998534a203972c"
FACTORIES = {
    "weighted": "0x8d88057b8cc9235d4c69db3c14fd8d4229a6ae35",
    "composable_stable": "0x60ef61c5c0f4213c7b2c02f64a575dc4bfdbf302",
    "managed": "0x08e3814ad06be2a555a4c0d330a20078e5344dfd",
}
REGISTRATION = "0x" + keccak(text="PoolRegistered(bytes32,address,uint8)").hex()
ENDPOINTS = ("https://base.gateway.tenderly.co", "https://mainnet.base.org")


class Directory:
    def __init__(self):
        self.http = requests.Session()

    def rpc(self, operations):
        result = []
        for pos in range(0, len(operations), 6):
            ops = operations[pos:pos + 6]
            payload = [dict(jsonrpc="2.0", id=i, method=method, params=params)
                       for i, (method, params) in enumerate(ops)]
            for attempt in range(4):
                try:
                    response = self.http.post(ENDPOINTS[attempt % 2], json=payload, timeout=25)
                    response.raise_for_status()
                    rows = {r["id"]: r for r in response.json()}
                    result.extend([rows[i]["result"] for i in range(len(ops))])
                    break
                except (requests.RequestException, KeyError, ValueError, TypeError) as err:
                    if attempt == 3:
                        raise RuntimeError(f"archive read {ops[0][0]}: {err}") from err
                    sleep(attempt + 1)
        return result

    def registrations(self, tip, slot):
        def nonce(heights):
            return [int(v, 16) for v in self.rpc([
                ("eth_getStorageAt", [VAULT, hex(slot), hex(h)]) for h in heights])]
        start, total = nonce([0, tip])
        if start or not total:
            raise ValueError("unexpected registration endpoints")
        windows = [(0, start, tip, total)]
        changes = []
        while windows:
            mids = [(lo + hi) // 2 for lo, _, hi, _ in windows]
            found = []
            for (lo, n, hi, m), mid, x in zip(windows, mids, nonce(mids), strict=True):
                if not n <= x <= m:
                    raise ValueError("non-monotone vault nonce")
                for l, a, h, b in [(lo, n, mid, x), (mid, x, hi, m)]:
                    if a != b:
                        (changes if h == l + 1 else found).append((l, a, h, b))
            windows = found
        pools = []
        for _, before, h, after in sorted(changes):
            logs, = self.rpc([("eth_getLogs", [dict(address=VAULT, fromBlock=hex(h),
                                toBlock=hex(h), topics=[REGISTRATION])])])
            batch = []
            for row in logs:
                pid = row["topics"][1].lower()
                pool = "0x" + row["topics"][2][-40:].lower()
                nonce_value = int(pid[-20:], 16)
                if pid[:42] != pool or row["removed"] or row["address"].lower() != VAULT:
                    raise ValueError("invalid registration log")
                batch.append(dict(pool=pool, id=pid, nonce=nonce_value, block=h, tx=row["transactionHash"],
                                  hash=row["blockHash"], specialization=int(row["data"], 16)))
            if sorted(p["nonce"] for p in batch) != list(range(before, after)):
                raise ValueError("pool registration sequence is incomplete")
            pools.extend(batch)
        return sorted(pools, key=lambda p: p["nonce"])


def capture():
    src = "https://sourcify.dev/server/v2/contract/8453/" + VAULT
    response = requests.get(src, params={"fields": "storageLayout,runtimeBytecode.onchainBytecode"}, timeout=25)
    response.raise_for_status()
    verified = response.json()
    slot, = [int(x["slot"]) for x in verified["storageLayout"]["storage"] if x["label"] == "_nextPoolNonce"]
    snap = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    directory = Directory()
    header, code = directory.rpc([("eth_getBlockByNumber", [hex(snap["block"]), False]),
                                  ("eth_getCode", [VAULT, snap["selector"]])])
    if header["hash"] != snap["selector"]["blockHash"] or code != verified["runtimeBytecode"]["onchainBytecode"]:
        raise ValueError("vault code differs from the verified deployment")
    pools = directory.registrations(snap["block"], slot)
    cap = Capture(dict(snap, endpoint=ENDPOINTS[1]))
    for k, f in FACTORIES.items():
        vault, = cap.one(f, "getVault()", returns=["address"])
        if vault != VAULT:
            raise ValueError(f"{k} points to a different vault")
    for p in pools:
        raw = bytes.fromhex(p["id"][2:])
        rows = posts(cap, [(f, "isPoolFromFactory(address)", ["address"], [p["pool"]]) for f in FACTORIES.values()]
                   + [(VAULT, "getPool(bytes32)", ["bytes32"], [raw]), (VAULT, "getPoolTokens(bytes32)", ["bytes32"], [raw])])
        p["families"] = [k for k, value in zip(FACTORIES, rows[:len(FACTORIES)], strict=True) if decode(["bool"], value)[0]]
        actual, specialization = decode(["address", "uint8"], rows[-2])
        if (actual, specialization) != (p["pool"], p["specialization"]):
            raise ValueError("registered pool or specialization changed")
        tokens, balances, last_change = decode(["address[]", "uint256[]", "uint256"], rows[-1])
        p.update(tokens=list(tokens), balances=list(map(str, balances)), last_change=last_change)
    return dict(chain=8453, selector=snap["selector"], vault=VAULT, factories=FACTORIES, verified=src, slot=slot,
                state_root=header["stateRoot"], pools=pools)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(capture(), indent=2) + "\n", encoding="utf8")
