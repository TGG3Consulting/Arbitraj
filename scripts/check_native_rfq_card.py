"""Проверка карточки Native RFQ Base по сборке solc и результату CREATE в GitHub Actions."""
import gzip
import hashlib
import json
import sys
from pathlib import Path

compiled = json.loads(Path(sys.argv[1]).read_bytes())
card = json.loads(Path("ui/src/assets/native-rfq-base-create.json").read_bytes())
directory = Path("tests/data/first_run_dex_2026-11-26")
archive = json.loads(gzip.decompress((directory/"native_rfq_base_wallets.json.gz").read_bytes()))
roles = json.loads((directory/"native_rfq_base_roles.json").read_bytes())["roles"]
constructor = next(m for m in compiled["abi"] if m["type"] == "constructor")
assert [v["type"] for v in constructor["inputs"]] == ["address","bytes32","address","bytes32"]
for key in ("router","pool"):
    assert card[key] == roles[key]["address"] and card[key+"Hash"] == roles[key]["runtime"]
suffix = "".join(card[k][2:].rjust(64,"0") for k in ("router","routerHash","pool","poolHash"))
assert card["data"] == "0x" + compiled["creation"] + suffix
assert hashlib.sha256(bytes.fromhex(card["data"][2:])).hexdigest() == card["creationSha256"]
runtime = bytes.fromhex(archive["runtime"][2:])
assert hashlib.sha256(runtime).hexdigest() == card["runtimeSha256"]
assert archive["block_hash"] == card["sourceBlock"]
assert {row["runtime"] for row in archive["observed"] if row["mode"] == "module"} == {card["runtimeKeccak"]}
# Патчим только диапазоны, объявленные solc как immutable, значениями из наблюдённого CREATE.
template = bytearray.fromhex(compiled["runtime"])
words = set()
for refs in compiled["immutableReferences"].values():
    values = set()
    for ref in refs:
        i = ref["start"]
        assert ref["length"] == 32
        values.add(runtime[i:i+32])
        template[i:i+32] = runtime[i:i+32]
    assert len(values) == 1
    words.update(values)
assert template == runtime and words == {bytes.fromhex(suffix[i:i+64]) for i in range(0,len(suffix),64)}
