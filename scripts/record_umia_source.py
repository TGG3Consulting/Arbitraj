"""Готовит исходники официального релиза Umia и сверяет скомпилированные GitHub байты с действующими spot hook."""
import argparse
import gzip
import json
import posixpath
import re
from pathlib import Path

import requests
from eth_utils import keccak
from record_umia_base import AT, DATA, call, many, read

PIN="3165f5a5bdaa23a83fbfd4894523e7ca84d53a14"
VERIFIED="0x128bedebaa9304db74726efba848a98e7fb53a80"
TARGETS=["UmiaHook","UmiaHookV2"]
PREFIX="src/periphery/"
BASE="https://raw.githubusercontent.com/umiafinance/protocol/"+PIN+"/"


def prepare(output):
    url=f"https://sourcify.dev/server/v2/contract/8453/{VERIFIED}?fields=sources,compilation,runtimeBytecode.onchainBytecode"
    reply=requests.get(url,timeout=20);reply.raise_for_status();v=reply.json()
    if v["runtimeMatch"] not in ("match","exact_match") or v["runtimeBytecode"]["onchainBytecode"].lower()!=read("eth_getCode",[VERIFIED,AT]):
        raise ValueError("использован чужой набор исходников")
    settings=v["compilation"]["compilerSettings"]
    if v["compilation"]["compilerVersion"]!="0.8.26+commit.8a97fa7a":raise ValueError("версия solc расходится")
    def resolve(path,source):
        if source.startswith("."):return posixpath.normpath(posixpath.join(posixpath.dirname(path),source))
        mapping=[r.split("=",1) for r in settings["remappings"] if ":" not in r.split("=")[0]]
        for prefix,target in sorted(mapping,key=lambda x:len(x[0]),reverse=True):
            if source.startswith(prefix):return target+source[len(prefix):]
        return source
    name=PREFIX+"UmiaHook.sol"
    ref_settings=dict(settings,outputSelection={name:{"UmiaHook":["evm.deployedBytecode.object","evm.deployedBytecode.immutableReferences"]}})
    reference={k:{"content":x["content"]} for k,x in v["sources"].items()}
    output.with_suffix(".reference.json").write_text(json.dumps(dict(language="Solidity",sources=reference,settings=ref_settings)),encoding="utf8")
    names=[PREFIX+t+".sol" for t in TARGETS]
    todo=list(names);sources={}
    while todo:
        path=todo.pop()
        if path in sources:continue
        if path.startswith("src/"):
            reply=requests.get(BASE+path,timeout=15);reply.raise_for_status();source=reply.text
        else:
            source=v["sources"][path]["content"]
        sources[path]={"content":source}
        for match in re.finditer(r'^\s*import\b[^;]*?["\']([^"\']+)["\']\s*;',source,re.M):
            todo.append(resolve(path,match[1]))
    settings["outputSelection"]={name:{"*":["evm.deployedBytecode.object","evm.deployedBytecode.immutableReferences"]} for name in names}
    output.write_text(json.dumps(dict(language="Solidity",sources=sources,settings=settings)),encoding="utf8")


def body(code):
    size=int.from_bytes(code[-2:],"big")
    if not 2<=size<=100 or code[-size-2] not in (0xa1,0xa2,0xa3):raise ValueError("неизвестные границы metadata")
    return code[:-size-2]


def verify(compiled,output):
    raw=compiled.read_text(encoding="utf8")
    product=json.loads(raw[raw.index("{"):])
    errors=[e["formattedMessage"][:500] for e in product.get("errors",[]) if e["severity"]=="error"]
    if errors:raise ValueError(errors[:3])
    raw=compiled.with_suffix(".reference.json").read_text(encoding="utf8")
    reference=json.loads(raw[raw.index("{"):])
    products=[(name,product) for name in TARGETS]+[("UmiaHook",reference)]
    with gzip.open(DATA/"umia_base_directory.json.gz") as f:directory=json.load(f)
    evidence=[]
    for hook in sorted({r["key"][-1] for r in directory["rows"] if "key" in r}):
        actual=bytes.fromhex(read("eth_getCode",[hook,AT])[2:])
        owner,=many([call(hook,"INITIAL_OWNER()","address")])
        matches=[]
        for name,build in products:
            deployed=build["contracts"][PREFIX+name+".sol"][name]["evm"]["deployedBytecode"]
            sample=bytearray.fromhex(deployed["object"]);refs=list(deployed["immutableReferences"].values())
            if len(refs)!=1:raise ValueError("исходник имеет иной состав immutable")
            for item in refs[0]:
                if item["length"]!=32:raise ValueError("immutable не address")
                start=item["start"];sample[start:start+32]=int(owner,16).to_bytes(32,"big")
            if body(actual)==body(sample):
                matches.append(dict(name=name,source="sourcify" if build is reference else PIN,metadata_equal=actual==sample,immutable=refs[0],logic_bytes=len(body(actual))))
        evidence.append(dict(hook=hook,initial_owner=owner,runtime_hash="0x"+keccak(actual).hex(),matches=matches))
    if not any(r["hook"]==VERIFIED and r["matches"] for r in evidence):raise ValueError("не воспроизвёлся эталон Sourcify")
    output.write_text(json.dumps(dict(selector=AT,pin=PIN,solc="0.8.26",hooks=evidence),indent=2),encoding="utf8")


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("action",choices=["prepare","verify"]);p.add_argument("path",type=Path);p.add_argument("output",nargs="?",type=Path)
    a=p.parse_args()
    if a.action=="prepare":prepare(a.path)
    else:verify(a.path,a.output)
