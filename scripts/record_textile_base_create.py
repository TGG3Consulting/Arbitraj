"""Карточка Textile Base. Два чтения конструктора на исходном блоке с разными owner; без подписей."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

DATA=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26"
def sha(data): return hashlib.sha256(data).hexdigest()

def capture(spec, url):
    snapshot=json.loads(gzip.decompress((DATA/"textile_base_wallets.json.gz").read_bytes()))
    tag=snapshot["selector"]
    def read(method,args):
        response=requests.post(url,json={"jsonrpc":"2.0","id":1,"method":method,"params":args},timeout=20)
        response.raise_for_status()
        return response.json()["result"]
    if int(read("eth_chainId",[]),16)!=8453 or read("eth_getBlockByNumber",[hex(62262756),False])["hash"]!=tag["blockHash"]:
        raise ValueError("Base canonical header")
    roles=snapshot["roles"]
    for role in roles.values():
        raw=bytes.fromhex(read("eth_getCode",[role["address"],tag])[2:])
        if len(raw)!=role["bytes"] or "0x"+keccak(raw).hex()!=role["runtime"]: raise ValueError("source role")
    reactor=roles["reactor"]; binary=bytes.fromhex(spec["creation"].removeprefix("0x"))
    template=bytes.fromhex(spec["runtime"].removeprefix("0x"))
    offsets={};controls=[]
    for owner in [snapshot["owner"],snapshot["makers"][0]]:
        words=encode(["address","address","bytes32"],[owner,reactor["address"],bytes.fromhex(reactor["runtime"][2:])])
        init=binary+words
        runtime=bytes.fromhex(read("eth_call",[{"from":owner,"data":"0x"+init.hex()},tag])[2:])
        masked=bytearray(runtime);target={}
        for refs in spec["immutableReferences"].values():
            values={runtime[f["start"]:f["start"]+f["length"]] for f in refs}
            if len(values)!=1 or any(f["length"]!=32 for f in refs):raise ValueError("immutable width")
            (value,)=values
            name=next((n for i,n in enumerate(("owner","reactor","reactorHash")) if value==words[i*32:i*32+32]),None)
            if name is None or name in target:raise ValueError("immutable role")
            target[name]=[f["start"] for f in refs]
            for at in target[name]:masked[at:at+32]=bytes(32)
        if set(target)!={"owner","reactor","reactorHash"} or masked!=template or (offsets and offsets!=target):
            raise ValueError("immutable template")
        offsets=target
        controls.append({"owner":owner.lower(),"creationSha256":sha(init),"runtimeSha256":sha(runtime),
                         "runtimeKeccak":"0x"+keccak(runtime).hex()})
    return {"kind":"textile_wallet","chainId":8453,"sourceBlock":tag["blockHash"],"roles":roles,
            "creation":"0x"+binary.hex(),"runtime":"0x"+template.hex(),"offsets":offsets,
            "creationTemplateSha256":sha(binary),"runtimeTemplateSha256":sha(template),"controls":controls}

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("compiled",type=Path);p.add_argument("output",type=Path);a=p.parse_args()
    spec=json.loads(a.compiled.read_bytes())
    for endpoint in ("https://mainnet.base.org","https://base.gateway.tenderly.co"):
        try:card=capture(spec,endpoint)
        except (requests.RequestException,KeyError,ValueError):
            if endpoint.endswith("tenderly.co"):raise
        else:
            a.output.write_text(json.dumps(card,separators=(",",":"))+"\n",encoding="utf8");break
