"""Указатель публичной фабрики Ring Ethereum по фиксированному каноническому хешу.
Перечисляет каждый allPairs(i), читает исходные токены, происхождение и резервы.
Файл справочника различает ненулевые резервы, покрытие underlying и реальный обмен из отдельного replay.
"""
import sys,json
import requests
from pathlib import Path
from eth_abi import encode,decode
from eth_utils import keccak
from record_base_slipstream_executor import calldata,rpc
from record_v2_chain_pools import view
from record_ring_ethereum import FACTORY,FEW,ROOT,QUOTER,WETH,PERMIT,ROUTER
MC = "0xca11bde05977b3631167028862be2a173976ca11"
PAIR_INIT = bytes.fromhex("a7ae6a5ec37f0c21bbdac560794258c4089b8ae3ffa6e3909b53c6091764a676")
FW_INIT = bytes.fromhex("2bdba5734ddf754fb149ef1faa937956c52cfd1f24d68163a95f42d08ec06d38")
PAIR_RUNTIME = "330be2c0f89c41b8879b8015445a667347764915107aced846fd148cc7c03bc0"
FW_RUNTIME = "be63f68c8de6e3f7cf411d9b117b931128468c36e0753fc5878fd5359f16acf5"

def create(factory,salt,init):
    return "0x"+keccak(b"\xff"+bytes.fromhex(factory[2:])+salt+init)[-20:].hex()

def bytes_address(raw):
    return decode(["address"],raw)[0] if raw and len(raw)==32 else None

def number(raw):
    return int.from_bytes(raw,"big") if raw and len(raw)==32 else None

def catalog(endpoint,tag):
    """Мультивызовы объединяют 80 чтений на одном адресе/блоке на HTTP-запрос."""
    def many(calls):
        result=[]
        for at in range(0,len(calls),80):
            part=calls[at:at+80]
            data=calldata("aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],
                          [[(a,True,bytes.fromhex(d[2:])) for a,d in part]])
            vals=decode(["(bool,bytes)[]"],view(endpoint,tag,MC,data))[0]
            result.extend(raw if ok else None for ok,raw in vals)
        return result
    def same(targets, signature):
        return many([(addr,calldata(signature)) for addr in targets])
    n=number(view(endpoint,tag,FACTORY,calldata("allPairsLength()")))
    pools=[bytes_address(x) for x in many([(FACTORY,calldata("allPairs(uint256)",["uint256"],[idx])) for idx in range(n)])]
    if None in pools or len(set(pools))!=n:raise ValueError("неполное перечисление allPairs")
    zero=[bytes_address(x) for x in same(pools,"token0()")]
    one=[bytes_address(x) for x in same(pools,"token1()")]
    reserves=same(pools,"getReserves()")
    fw=sorted(set(zero+one))
    underlying=[bytes_address(x) for x in same(fw,"token()")]
    factories=[bytes_address(x) for x in same(fw,"factory()")]
    wrapped_decimals=[number(x) for x in same(fw,"decimals()")]
    unique=list(dict.fromkeys(a for a in underlying if a))
    precision=dict(zip(unique,[number(x) for x in same(unique,"decimals()")],strict=True))
    canonical=dict(zip(unique,[bytes_address(x) for x in many([(FEW,calldata("getWrappedToken(address)",["address"],[a])) for a in unique])],strict=True))
    backing=many([(a,calldata("balanceOf(address)",["address"],[w])) if a else (FEW,calldata("allWrappedTokensLength()")) for w,a in zip(fw,underlying)])
    tokens={}
    for w,a,f,d,b in zip(fw,underlying,factories,wrapped_decimals,backing,strict=True):
        reason = None
        if not a:reason="no_underlying"
        elif f!=FEW:reason="different_wrapper_factory"
        elif canonical[a]!=w:reason="not_canonical_wrapper"
        elif w!=create(FEW,keccak(encode(["address"],[a])),FW_INIT):reason="wrapper_create2_mismatch"
        elif d is None or d>77 or precision[a]!=d:reason="different_or_unsupported_decimals"
        elif number(b) is None:reason="unreadable_backing"
        tokens[w]={"asset":a,"factory":f,"canonical":canonical.get(a),"decimals":d,
                   "backing":None if not a or number(b) is None else str(number(b)),"verified":reason is None,"reason":reason}
    registered=many([(FACTORY,calldata("getPair(address,address)",["address","address"],[a,b])) for a,b in zip(zero,one)])
    rows=[]
    for i,(p,a,b,res,reg) in enumerate(zip(pools,zero,one,reserves,registered,strict=True)):
        valid=(a<b and bytes_address(reg)==p and create(FACTORY,keccak(bytes.fromhex(a[2:]+b[2:])),PAIR_INIT)==p
               and tokens[a]["verified"] and tokens[b]["verified"])
        values=decode(["uint112","uint112","uint32"],res)[:2] if res else (0,0)
        rows.append({"index":i,"pool":p,"wrappers":[a,b],"reserves":list(map(str,values)),"verified":valid})
    # Полный байткод передаётся только в ответе read-only RPC и хешируется локально.
    admitted=[p for p in rows if p["verified"]]
    expected={p["pool"]:PAIR_RUNTIME for p in admitted}
    expected.update({w:FW_RUNTIME for p in admitted for w in p["wrappers"]})
    code_hash={}
    addresses=sorted(expected)
    for n in range(0,len(addresses),3):
        group=addresses[n:n+3]
        payload=[{"jsonrpc":"2.0","id":i,"method":"eth_getCode","params":[a,tag]} for i,a in enumerate(group)]
        response=requests.post(endpoint,json=payload,timeout=30)
        response.raise_for_status()
        by_id={row["id"]:row for row in response.json()}
        for i,a in enumerate(group):
            runtime=bytes.fromhex(by_id[i]["result"][2:])
            code_hash[a]="0x"+keccak(runtime).hex()
    for p in admitted:
        if any(code_hash[a]!="0x"+expected[a] for a in [p["pool"],*p["wrappers"]]):
            p["verified"]=False
            p["reason"]="runtime_mismatch"
    return {"code_hash":code_hash, "selector":tag,"factory":FACTORY,"few":FEW,"pair_init":"0x"+PAIR_INIT.hex(),"wrapper_init":"0x"+FW_INIT.hex(),
            "count":n,"tokens":tokens,"rows":rows}

def main(filename):
    s=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_text())
    if int(rpc(s["endpoint"],"eth_chainId",[]),16)!=1: raise ValueError("чужая сеть")
    j=catalog(s["endpoint"],s["selector"]);j["chain_id"]=1;j["block"]=s["block"]
    Path(filename).write_bytes((json.dumps(j,separators=(",",":"))+"\n").encode())

if __name__=="__main__":
    main(sys.argv[1])
