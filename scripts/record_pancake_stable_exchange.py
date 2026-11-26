"""Сверяет get_dy с переводом после exchange в изолированной исторической eth_simulateV1 Ethereum."""
import requests
from eth_abi import encode
from eth_utils import keccak

from record_balancer_deployments import ROOT, read, write

HOLDERS = {
    # Публичные EOA из eth.blockscout.com/api/v2/tokens/{token}/holders; баланс проверяется самой симуляцией.
    "0xd48067f122afc3a58f0f79611f5f1afae0d7f25b": "0xefbe864256d7418f6e15ea39b840b4bd9d5a5817",
    "0xc43c6bfeda065fe2c4c11765bf838789bd0bb5de": "0xba8e11067cfe473529eb39671562b1a2b991be56",
    "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599": "0x176f3dab24a159341c0509bb36b833e7fdd0a132",
    "0xbdf245957992bfbc62b07e344128a1eec7b7ee3f": "0xe87c40efee0b2b2245254d67aa74d6cf620eefe0",
}


def data(signature, types, values):
    return "0x" + (keccak(text=signature)[:4] + encode(types, values)).hex()


def simulate(profile, pool, quote, expected):
    source, target = (pool["coins"][quote[k]] for k in ("i", "j"))
    sender = HOLDERS[source]
    dx = int(quote["dx"])
    addr = pool["pool"]
    args = [quote["i"], quote["j"], dx]
    balance = data("balanceOf(address)", ["address"], [sender])
    unsigned = ["uint256"] * 3
    approve = data("approve(address,uint256)", ["address", "uint256"], [addr, dx])
    getter = data("get_dy(uint256,uint256,uint256)", unsigned, args)
    def swap(limit):
        return data("exchange(uint256,uint256,uint256,uint256)", unsigned + ["uint256"], args + [limit])
    cases = [(source, approve), (addr, getter), (source, balance), (target, balance)]
    if expected is not None:
        cases.append((addr, swap(expected + 1)))
    cases.extend([(addr, swap(expected or 0)), (source, balance), (target, balance)])
    calls = [{"from": sender, "to": to, "data": calldata} for to, calldata in cases]
    params = [{"blockStateCalls": [{"calls": calls}], "validation": False}, profile["selector"]]
    payload = {"jsonrpc": "2.0", "id": 1, "method": "eth_simulateV1", "params": params}
    response = requests.post(profile["endpoint"], json=payload, timeout=25)
    response.raise_for_status()
    reply = response.json()
    if "result" not in reply:
        raise ValueError(reply.get("error"))
    block, = reply["result"]
    results = block["calls"]
    if block["parentHash"] != profile["selector"]["blockHash"] or len(results) != len(calls):
        raise ValueError("неожиданный блок или неполный ответ")
    for idx, result in enumerate(results):
        negative = expected is not None and idx == 4
        if result["status"] != ("0x0" if negative else "0x1"):
            raise ValueError(f"{addr} {source} {idx}: {result}")
        revert = result.get("error", {}).get("data") or result["returnData"]
        if negative and "Exchange resulted in fewer coins than expected" not in bytes.fromhex(
                revert[2:]).decode("utf8", errors="ignore"):
            raise ValueError(f"неожиданная причина отказа: {result}")
    integer = lambda i: int(results[i]["returnData"], 16)
    received = integer(-1) - integer(3)
    if integer(2) - integer(-2) != dx or integer(1) != int(quote["dy"]) or received <= 0:
        raise ValueError("расхождение балансов либо исходного get_dy")
    if expected is not None and received != expected:
        raise ValueError("повторная симуляция дала другую сумму")
    return received, dict(request=payload, calls=results, parent_hash=block["parentHash"])


def main():
    folder = ROOT / "tests/data/first_run_dex_2026-11-26"
    metadata = read(folder / "ethereum.json")
    snapshot = read(folder / "pancake_stable_ethereum.json")
    cases = []
    for pool in snapshot["markets"]:
        for quote in pool["quotes"]:
            amount, _ = simulate(metadata, pool, quote, None)
            _, raw = simulate(metadata, pool, quote, amount)
            cases.append(dict(pool=pool["pool"], i=quote["i"], j=quote["j"], dx=quote["dx"],
                              get_dy=quote["dy"], exchange_out=str(amount), **raw))
    write(folder / "pancake_stable_exchange_ethereum.json",
          dict(chain_id=1, selector=metadata["selector"], endpoint=metadata["endpoint"], cases=cases))


if __name__ == "__main__":
    main()
