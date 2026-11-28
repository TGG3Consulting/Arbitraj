"""РџСЂРѕРІРµСЂРєР° РєР°СЂС‚РѕС‡РєРё Native RFQ Base и Ethereum РїРѕ СЃР±РѕСЂРєРµ solc Рё СЂРµР·СѓР»СЊС‚Р°С‚Сѓ CREATE РІ GitHub Actions."""
import gzip
import hashlib
import json
import sys
from pathlib import Path

compiled = json.loads(Path(sys.argv[1]).read_bytes())
constructor = next(m for m in compiled["abi"] if m["type"] == "constructor")
assert [v["type"] for v in constructor["inputs"]] == ["address","bytes32","address","bytes32"]

directory = Path("tests/data/first_run_dex_2026-11-26")
def load_archive(name):
    return json.loads(gzip.decompress((directory/name).read_bytes()))

def verify(network, roles, runtime_hex, block_hash, observed):
    card = json.loads(Path(f"ui/src/assets/native-rfq-{network}-create.json").read_bytes())
    for key in ("router","pool"):
        assert card[key] == roles[key]["address"] and card[key+"Hash"] == roles[key]["runtime"]
    suffix = "".join(card[k][2:].rjust(64,"0") for k in ("router","routerHash","pool","poolHash"))
    assert card["data"] == "0x" + compiled["creation"] + suffix
    assert hashlib.sha256(bytes.fromhex(card["data"][2:])).hexdigest() == card["creationSha256"]
    runtime = bytes.fromhex(runtime_hex[2:])
    assert hashlib.sha256(runtime).hexdigest() == card["runtimeSha256"]
    assert block_hash == card["sourceBlock"]
    assert {row["runtime"] for row in observed} == {card["runtimeKeccak"]}
    # РџР°С‚С‡РёРј С‚РѕР»СЊРєРѕ РґРёР°РїР°Р·РѕРЅС‹, РѕР±СЉСЏРІР»РµРЅРЅС‹Рµ solc РєР°Рє immutable, Р·РЅР°С‡РµРЅРёСЏРјРё РёР· РЅР°Р±Р»СЋРґС‘РЅРЅРѕРіРѕ CREATE.
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

base = load_archive("native_rfq_base_wallets.json.gz")
base_roles = json.loads((directory/"native_rfq_base_roles.json").read_bytes())["roles"]
verify("base", base_roles, base["runtime"], base["block_hash"], [row for row in base["observed"] if row["mode"]=="module"])
ethereum = load_archive("native_rfq_ethereum_create.json.gz")
captures = load_archive("native_rfq_ethereum_protected.json.gz")["observed"]
eth_roles = json.loads((directory/"native_rfq_ethereum_public.json").read_bytes())["roles"]["roles"]
assert ethereum["blockHash"] in {row["head"]["hash"] for row in captures}
assert int(ethereum["registry"]["response"]["result"],16) == 1
verify("ethereum", dict(zip(("router","pool"),eth_roles,strict=True)), ethereum["runtime"], ethereum["blockHash"], captures)
