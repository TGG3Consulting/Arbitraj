"""Identify the Vyper executable prefix and constructor immutables from CODECOPY/RETURN on Ethereum."""
import collections
import requests
from eth_utils import keccak

from record_llama_base_cl import CanonicalCalls


def template(init, deployed):
    offset=init.find(deployed[:64])
    if offset<0:raise ValueError("creation has no deployed program")
    instructions=[]
    i=0
    while i<offset:
        op=init[i];n=op-95 if 96<=op<=127 else 0
        value=int.from_bytes(init[i+1:i+1+n],"big") if n or op==0x5f else None
        instructions.append((op,value));i+=1+n
    matches=[]
    for i,(op,_) in enumerate(instructions):
        if op!=0xf3 or i<6:continue
        size,source,dest,copy,total,begin,_=instructions[i-6:i+1]
        if None in (size[1],source[1],dest[1],total[1],begin[1]):continue
        if source[1]==offset and dest[1]==begin[1]==0 and copy[0]==0x39 and total[1]==len(deployed):
            count=size[1];suffix=len(deployed)-count
            if 0<count<len(deployed) and suffix%32==0 and init[offset:offset+count]==deployed[:count]:
                matches.append((count,suffix))
    if len(matches)!=1:raise ValueError("ambiguous runtime return")
    return matches[0]


def capture(reader, book, factories):
    cap=CanonicalCalls(reader);result={}
    for kind,markets in book.items():
        getter={"mint":"amm_implementation()","oneway":"amm_impl()","v2":"amm_blueprint()"}[kind]
        blueprint,=cap.one(factories[kind],getter,returns=["address"])
        blueprint_code=reader.one("eth_getCode",[blueprint,reader.tag])
        classes=collections.defaultdict(list);all_code={}
        for start in range(0,len(markets),5):
            batch=markets[start:start+5]
            raw=reader.fetch([("eth_getCode",[m["amm"],reader.tag]) for m in batch])
            for m,v in zip(batch,raw,strict=True):
                code=bytes.fromhex(v[2:]);all_code[m["amm"]]=code
                classes[len(code)].append(m)
        profiles=[]
        for size,group in classes.items():
            # Repeat for classes sharing a length but having different bytecode.
            remaining=list(group)
            while remaining:
                sample=remaining[0];address=sample["amm"];deployed=all_code[address]
                url="https://eth.blockscout.com/api/v2/smart-contracts/"+address
                res=requests.get(url,timeout=30);res.raise_for_status();j=res.json()
                if j.get("deployed_bytecode","").lower()!="0x"+deployed.hex():
                    raise ValueError(("explorer/archived mismatch",address))
                prefix,imm=template(bytes.fromhex(j["creation_bytecode"][2:]),deployed)
                code_hash="0x"+keccak(deployed[:prefix]).hex()
                proved=[]
                for m in remaining:
                    code=all_code[m["amm"]]
                    if code[:prefix]!=deployed[:prefix]:continue
                    words=[int.from_bytes(code[i:i+32],"big") for i in range(prefix,size,32)]
                    if len(words)<4 or [words[0],words[2]]!=[int(m["borrowed"],16),int(m["collateral"],16)]:
                        raise ValueError(("immutable cash",m["amm"]))
                    proved.append(dict(amm=m["amm"],runtime="0x"+keccak(code).hex()))
                confirmed={m["amm"] for m in proved}
                remaining=[m for m in remaining if m["amm"] not in confirmed]
                profiles.append(dict(executable=prefix,immutableBytes=imm,runtimeBytes=size,bodyKeccak=code_hash,
                                     source=url,verified=bool(j.get("is_verified")),markets=proved))
        result[kind]=dict(factoryKeccak="0x"+keccak(hexstr=reader.one("eth_getCode",[factories[kind],reader.tag])).hex(),
                          blueprint=blueprint,blueprintKeccak="0x"+keccak(hexstr=blueprint_code).hex(),classes=profiles)
    return result

