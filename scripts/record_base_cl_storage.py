"""Prove complete Base CL factory storage, including fee tiers with zero deployed pools."""
import argparse
import gzip
import json
import re
from pathlib import Path

import requests
from eth_abi import decode

from fork_fee_rpc import Reader
from record_balancer_markets import posts
from record_base_slipstream_executor import ROOT
from record_llama_base_cl import CanonicalCalls
from record_v3_fee_registry import candidate_fees
from v3_factory_storage import verify_directory


def directory(name):
    fixtures = ROOT / "tests/data/first_run_dex_2026-11-26"
    head = json.loads((fixtures / "base.json").read_bytes())
    if name == "kinetix":
        book = json.loads(gzip.decompress((fixtures / "kinetix_v3_base.json.gz").read_bytes()))
    else:
        books = json.loads(gzip.decompress((fixtures / "wagmi_thick_base.json.gz").read_bytes()))
        book = books[name]
    factory = book["roles"]["factory"]["address"]
    reader = Reader("base", head)
    cap = CanonicalCalls(reader)
    proof = reader.one("eth_getProof", [factory, [], reader.tag])
    assert book["selector"] == reader.tag and proof["codeHash"] == book["roles"]["factory"]["runtime"]

    url = f"https://sourcify.dev/server/v2/contract/8453/{factory}"
    res = requests.get(url, params={"fields": "storageLayout,sources,compilation,runtimeBytecode.onchainBytecode"}, timeout=30)
    res.raise_for_status()
    compiled = res.json()
    assert compiled["runtimeBytecode"]["onchainBytecode"].lower() == reader.one("eth_getCode", [factory, reader.tag])
    layout = {row["label"]: int(row["slot"]) for row in compiled["storageLayout"]["storage"]}
    assert all(row["offset"] == 0 for row in compiled["storageLayout"]["storage"])
    assert set(layout) == ({"parameters", "owner", "feeAmountTickSpacing", "getPool"} |
                           ({"allPairs", "feeProtocol"} if name == "thick" else set()))
    source_path = compiled["compilation"]["fullyQualifiedName"].rsplit(":", 1)[0]
    source = compiled["sources"][source_path]["content"]
    initial = set(map(int, re.findall(r"feeAmountTickSpacing\[(\d+)\]\s*=", source)))
    catalog = json.loads((ROOT / "config/first_run_dex.json").read_bytes())
    fees = sorted(initial | candidate_fees(catalog, fixtures) | {m["key"] for m in book["markets"]})
    raw = posts(cap, [(factory, "feeAmountTickSpacing(uint24)", ["uint24"], [fee]) for fee in fees])
    tiers = {fee: s for fee, row in zip(fees, raw, strict=True) if (s := decode(["int24"], row)[0]) > 0}
    owner, = cap.one(factory, "owner()", returns=["address"])
    pools = [dict(pool=m["pool"], tokens=m["tokens"], fee=m["key"]) for m in book["markets"]]
    assert all(tiers[m["key"]] == m["spacing"] for m in book["markets"])
    slots = int(int(owner, 16) > 0) + len(tiers) + 2 * len(pools)
    record = dict(chain_id=8453, factory=factory, selector=reader.tag, state_root=reader.header["stateRoot"],
                  account_proof=proof["accountProof"], nonce=int(proof["nonce"], 16),
                  storage_root=proof["storageHash"], layout=layout, owner=owner, tiers=tiers, pools=pools,
                  creation_fees=sorted(initial), source=url, runtime=proof["codeHash"])
    if "allPairs" in layout:
        # THICK exports the indexed getter only. The array length is stored in its compiler layout slot.
        size = int(reader.one("eth_getStorageAt", [factory, hex(layout["allPairs"]), reader.tag]), 16)
        assert size == len(pools)
        values = posts(cap, [(factory, "allPairs(uint256)", ["uint256"], [i]) for i in range(size)])
        record["all_pairs"] = [decode(["address"], value)[0] for value in values]
        record["fee_protocol"], = cap.one(factory, "feeProtocol()", returns=["uint8"])
        slots += size + 1 + int(record["fee_protocol"] > 0)
    record["storage_slots"] = slots
    verify_directory(record, factory)
    return record


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("name", choices=["wagmi", "thick", "kinetix"])
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_bytes(json.dumps(directory(args.name), separators=(",", ":")).encode())
