"""Match the Ethereum CREATE card to the compiled bytecode and the canonical settlement archive."""
import gzip
import hashlib
import json
import sys
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

fixture = Path("tests/data/first_run_dex_2026-11-26")
observed = json.loads(gzip.decompress((fixture / "zeroex_ethereum_wallet.json.gz").read_bytes()))
card = json.loads(Path("ui/src/assets/zeroex-ethereum-create.json").read_bytes())
compiled = json.loads(Path(sys.argv[1]).read_bytes())
functions = {x["name"] for x in compiled["abi"] if x["type"] == "function"}
assert functions == {"settle","holder","holderHash","registry","registryHash"}
roles = [card["holder"], card["holderHash"], card["registry"], card["registryHash"]]
words = encode(["address","bytes32","address","bytes32"], [bytes.fromhex(s[2:]) for s in roles])
assert card["data"] == observed["creation"] == "0x" + compiled["creation"] + words.hex()
assert card["sourceBlock"] == observed["selector"]["blockHash"] and observed["selector"]["requireCanonical"]
assert observed["holder"] == roles[0] and observed["registry"] == roles[2]
assert [observed["codeHashes"][addr] for addr in roles[::2]] == roles[1::2]
runtime = bytes.fromhex(observed["runtime"][2:])
template = bytearray.fromhex(compiled["runtime"])
actual = set()
for sites in compiled["immutableReferences"].values():
    values = set()
    for site in sites:
        index=site["start"]
        assert site["length"]==32
        word = runtime[index:index+32]
        template[index:index+32]=word
        values.add(word)
    assert len(values)==1
    actual.update(values)
assert actual == {words[i:i+32] for i in range(0,len(words),32)} and template == runtime
assert hashlib.sha256(bytes.fromhex(card["data"][2:])).hexdigest() == card["creationSha256"]
assert hashlib.sha256(runtime).hexdigest()==card["runtimeSha256"] and "0x"+keccak(runtime).hex()==card["runtimeKeccak"]
