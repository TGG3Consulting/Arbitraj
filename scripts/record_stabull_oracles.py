"""Публичные курсы всех assimilator Stabull на двух закреплённых блоках Base."""
import gzip
import json

from record_stabull_base import DATA, batch, rpc


def record():
    directory = json.loads(gzip.decompress((DATA / "stabull_base_directory.json.gz").read_bytes()))
    registered = {}
    for pool in directory["pools"]:
        for i in [0, 1]:
            address = pool[f"assimilator{i}"]
            token = pool[f"token{i}"]
            if registered.setdefault(address, token) != token:
                raise ValueError("Assimilator отвечает за две разные валюты")
    samples = []
    for height in [directory["start"], directory["height"]]:
        block = rpc("eth_getBlockByNumber", [hex(height), False])
        selector = {"blockHash": block["hash"], "requireCanonical": True}
        calls = [(address, method, (), abi) for address in registered
                 for method, abi in [("token()", "address"), ("oracle()", "address"),
                                     ("getRate()", "uint256"), ("oracleDecimals()", "uint256")]]
        values = batch(selector, calls)
        roles = []
        for i, (address, token) in enumerate(registered.items()):
            asset, oracle, rate, scale = values[i*4:i*4+4]
            roles.append({"assimilator": address, "expected_token": token, "token": asset,
                          "oracle": oracle, "rate": rate, "oracle_scale": scale})
        observed = [row for row in roles if row["oracle"] and int(row["oracle"], 16) != 0]
        reads = [(row["oracle"], "latestRoundData()", (), "uint80,int256,uint256,uint256,uint80") for row in observed]
        for row, round_data in zip(observed, batch(selector, reads), strict=True):
            row["round"] = round_data
        samples.append({"height": height, "selector": selector, "timestamp": int(block["timestamp"], 16),
                        "state_root": block["stateRoot"], "roles": roles})
    assert samples[-1]["selector"] == directory["anchor"]
    return {"chain": 8453, "source": directory["source"], "samples": samples}


if __name__ == "__main__":
    (DATA / "stabull_base_oracles.json").write_text(json.dumps(record(), indent=2) + "\n", encoding="utf8", newline="\n")
