"""Сопоставляет оба Quoter, живые менеджеры и идентификатор индексатора с точными PoolId."""
import argparse
import json
from pathlib import Path
from eth_abi import decode
from eth_utils import keccak
from record_base_launch_bindings import read

def record(folder):
    directory=json.loads((folder/"omni-directory.json").read_bytes())
    at=directory["selector"];manager=directory["roles"]
    constructor=json.loads((folder.parent/"base-36/omni-source-router.json").read_bytes())["creationBytecode"]["transformationValues"]["constructorArguments"]
    args=decode(["address"]*5+["bytes32"]*2+["address"]*6,bytes.fromhex(constructor[2:]))
    assert (args[7],args[8],args[9])==(manager["vault"],manager["clPoolManager"],manager["binPoolManager"])
    roles=dict(permit2=args[0],wrapped=args[1],router=directory["router"],**manager)
    observations=[]
    for path in sorted(folder.glob("omni-flux-gecko-keys-*.json")):
        observations.extend((r["attributes"]["address"].lower(),r["relationships"]["dex"]["data"]["id"]) for r in json.loads(path.read_bytes())["data"])
    if dict(observations)!={m["id"]:"omni-exchange-v4-base" for m in directory["markets"]}:
        raise ValueError("индексатор расходится с физическими рынками")
    calls=[]
    def getter(contract,sig):
        data="0x"+keccak(text=sig)[:4].hex()
        result=read("eth_call",[dict(to=contract,data=data),at])
        calls.append(dict(contract=contract,data=data,sig=sig,result=result))
        return decode(["address"],bytes.fromhex(result[2:]))[0]
    for name,key in [("CLQuoter","clPoolManager"),("BinQuoter","binPoolManager")]:
        source=json.loads((folder/("omni-neighbor-"+name+".json")).read_bytes())
        addr=source["address"].lower()
        if getter(addr,"poolManager()")!=roles[key] or getter(addr,"vault()")!=roles["vault"]:
            raise ValueError("неверные роли "+name)
        roles[name]=addr
    code={}
    for k,a in roles.items():
        raw=bytes.fromhex(read("eth_getCode",[a,at])[2:])
        if not raw:raise ValueError("пустой код роли "+k)
        code[a]=dict(bytes=len(raw),keccak256="0x"+keccak(raw).hex())
    return dict(selector=at,roles=roles,calls=calls,code=code,indexer=dict(observations),router_constructor=constructor,
                position_source="https://github.com/omni-exchange/smart-contracts-public/tree/9e046fa9dc37/projects/flux-periphery",
                cl_quoter_source="https://basescan.org/address/"+roles["CLQuoter"]+"#code")
if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("folder",type=Path);ap.add_argument("output",type=Path);a=ap.parse_args()
    a.output.write_text(json.dumps(record(a.folder),indent=2)+"\n",encoding="utf8")
