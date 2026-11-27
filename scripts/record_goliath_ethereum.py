"""Реестр Goliath и доступность одного обычного плательщика до/после границы блока. Только чтение."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_bitswap_ethereum import ENDPOINTS, OWNER, WETH, tx

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
METHODS = {"eth_chainId", "eth_getBlockByHash", "eth_getCode", "eth_call", "eth_simulateV1", "eth_getStorageAt", "eth_getBalance"}


def query(method, params, allow_revert=False):
    if method not in METHODS:
        raise ValueError(method)
    errors = []
    for endpoint in ENDPOINTS:
        try:
            r = requests.post(endpoint, json=dict(jsonrpc="2.0", id=1, method=method, params=params), timeout=20)
            r.raise_for_status()
            reply = r.json()
            if "result" in reply:
                return reply["result"]
            error = reply["error"]
            if allow_revert and error.get("code") == 3:
                return {"error": error}
            errors.append(str(error)[:180])
        except (ValueError, TypeError, KeyError, requests.RequestException) as e:
            errors.append(str(e)[:180])
    raise RuntimeError((method, errors))


def calls(venue, markets):
    rows = []
    for market in markets:
        a, b = market["tokens"]
        vault = market["vault"]
        rows.append(("venue_active", vault, tx(venue, "isActive(address,address)", ["address", "address"], [a, b])))
        rows.append(("vault_active", vault, tx(vault, "isActive()")))
        for src, dst in ((a, b), (b, a)):
            for qty in ((10**14, 10**16) if src == WETH else (10**6, 10**7)):
                quote = tx(venue, "quote(address,address,uint256)", ["address", "address", "uint256"], [src, dst, qty])
                rows.append(("quote", (src, dst, qty), quote))
    return rows


def observe(seed, head):
    selector = head["selector"]
    block = query("eth_getBlockByHash", [selector["blockHash"], False])
    if int(block["number"], 16) != head["height"] or block["hash"] != selector["blockHash"]:
        raise ValueError("канонический блок")
    venue = seed["venue"]
    code = query("eth_getCode", [venue, selector])
    if "0x" + keccak(hexstr=code).hex() != seed["runtime"]:
        raise ValueError("код Goliath")
    raw = query("eth_call", [tx(venue, "getPairs()"), selector])
    pairs = decode(["(address,address)[]"], bytes.fromhex(raw[2:]))[0]
    if pairs != tuple(tuple(r["tokens"]) for r in seed["markets"]):
        raise ValueError("реестр пар")
    for market in seed["markets"]:
        getter = tx(venue, "getVault(address,address)", ["address", "address"], market["tokens"])
        actual = decode(["address"], bytes.fromhex(query("eth_call", [getter, selector])[2:]))[0]
        vault_code = query("eth_getCode", [actual, selector])
        if actual != market["vault"] or "0x" + keccak(hexstr=vault_code).hex() != market["runtime"]:
            raise ValueError("привязка и код vault")
    cases = calls(venue, seed["markets"])
    messages = [dict(r, gas=hex(500000)) for _, _, r in cases]
    if any(v["from"].lower() != OWNER.lower() for v in messages):
        raise ValueError("плательщик")
    at = [query("eth_call", [v, selector], allow_revert=True) for v in messages]
    # Обычный следующий блок строится из исходного состояния указанного родителя.
    # Настройки блока, цены поставщика и данные внешнего кошелька не меняются.
    blocks = query("eth_simulateV1", [{"blockStateCalls": [{"calls": messages}]}, selector])
    if len(blocks) != 1 or blocks[0]["parentHash"] != selector["blockHash"]:
        raise ValueError("связь дочернего блока")
    observed = blocks[0]["calls"]
    if len(observed) != len(cases):
        raise ValueError("неполная симуляция")
    return dict(parent=head, timestamp=block["timestamp"], child=blocks[0]["number"], child_timestamp=blocks[0]["timestamp"],
                rows=[dict(method=k, args=p, head=x, child=y) for (k, p, _), x, y in zip(cases, at, observed, strict=True)])


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output", type=Path)
    args = cli.parse_args()
    if query("eth_chainId", []) != "0x1":
        raise ValueError("нужен Ethereum")
    seed = json.loads((DATA / "goliath_ethereum.json").read_bytes())
    report = {"payer": OWNER, "registry": seed, "states": [observe(seed, h) for h in seed["heads"]]}
    from record_goliath_wallets import collect
    for state in report["states"]:
        state["wallet"] = collect(seed, state, query)
    args.output.write_text(json.dumps(report, separators=(",", ":")), encoding="utf8")
