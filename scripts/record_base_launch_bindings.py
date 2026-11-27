"""Сверяет код и стандартные слоты реализации зависимостей Base launch hooks на каноническом хеше."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
from eth_utils import keccak
from record_base_launch_hooks import ENDPOINTS

def read(method,params):
    failure=None
    for ep in ENDPOINTS:
        try:
            res=requests.post(ep,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=20)
            res.raise_for_status()
            payload=res.json()
            if "result" not in payload:raise ValueError(str(payload.get("error"))[:200])
            return payload["result"]
        except (ValueError,requests.RequestException) as exc:failure=exc
    raise ValueError(f"{method}: {failure}")

def record(directory):
    at=directory["selector"]
    addresses=set(directory["contracts"])
    kinds=("after_hook","registry","escrow","factory","airlock")
    for row in directory["rows"]:
        t=row.get("terms",{})
        addresses.update(t[k] for k in kinds if k in t)
    def code(addr):
        raw=bytes.fromhex(read("eth_getCode",[addr,at])[2:])
        if not raw:raise ValueError("роль без кода "+addr)
        return {"hash":"0x"+keccak(raw).hex(),"size":len(raw)}
    def contract(addr):
        values=code(addr)
        storage={}
        for name in ("implementation","beacon"):
            slot=hex(int.from_bytes(keccak(text="eip1967.proxy."+name))-1)
            raw=read("eth_getStorageAt",[addr,slot,at])
            storage[name]=raw
            if int(raw,16):
                child="0x"+raw[-40:]
                values[name]=dict(address=child,**code(child))
        values["storage"]=storage
        return addr,values
    with ThreadPoolExecutor(4) as pool:
        return dict(pool.map(contract,sorted(addresses)))

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input",type=Path);parser.add_argument("output",type=Path)
    args=parser.parse_args()
    args.output.write_text(json.dumps(record(json.loads(args.input.read_bytes())),indent=2)+"\n",encoding="utf8")
