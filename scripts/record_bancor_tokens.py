"""Pin the audited, non-proxy ERC20 implementations eligible for Bancor's direct nominal minimum."""
import hashlib
import json
from pathlib import Path

import requests
from eth_utils import keccak

ROOT=Path(__file__).resolve().parents[1]
TOKENS={
    "BNT":"0x1f573d6fb3f13d689ff844b4ce37794d79a7ff1c",
    "BAT":"0x0d8775f648430679a709e98d2b0cb6250d2887ef",
    "LINK":"0x514910771af9ca656af840dff83e8264ecf986ca",
}
def record():
    directory=ROOT/"tests/data/first_run_dex_2026-11-26"
    snapshot=json.loads((directory/"ethereum.json").read_bytes())
    roles={}
    for name,token in TOKENS.items():
        url="https://eth.blockscout.com/api/v2/smart-contracts/"+token
        info=requests.get(url,timeout=20);info.raise_for_status();verified=info.json()
        reply=requests.post(snapshot["endpoint"],json=dict(id=1,jsonrpc="2.0",method="eth_getCode",params=[token,snapshot["selector"]]),timeout=20)
        reply.raise_for_status();code=bytes.fromhex(reply.json()["result"][2:])
        if (not code or not verified.get("is_verified") or verified.get("is_changed_bytecode") or verified.get("implementations")
                or code!=bytes.fromhex(verified["deployed_bytecode"].removeprefix("0x"))):
            raise ValueError("Unverified direct ERC20 "+name)
        roles[token]=dict(symbol=name,bytes=len(code),keccak256="0x"+keccak(code).hex(),source=url,
                          verified_source_sha256=hashlib.sha256(verified["source_code"].encode()).hexdigest(),
                          compiler=verified["compiler_version"],full_source_match=bool(verified.get("is_fully_verified")))
    file=directory/"bancor_v3_exact_tokens.json"
    file.write_text(json.dumps(dict(selector=snapshot["selector"],tokens=roles),indent=2)+"\n",encoding="utf8")

if __name__=="__main__":
    record()
