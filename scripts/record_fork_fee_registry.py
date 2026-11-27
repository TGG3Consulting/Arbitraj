"""Проверяет все тарифы iZi/BitSwap и Kyber Elastic по корню хранилища на каноническом блоке.

Положительный getter фиксирует величину. Равенство полного trie доказывает, что на H
отсутствуют неизвестные тарифы, включая никогда не использованные в пулах.
"""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
import rlp

from fork_fee_rpc import Reader, pool_blocks
from v3_factory_storage import map_slot, nibbles, prove_account, trie_node, word


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests/data/first_run_dex_2026-11-26"
# Проверенные контрактные файлы: семейства различаются событием регистрации и набором storage slots.
SPECS = {
    "iziswap-base": ("izi", 1995942, "7e5ff9f6a7e8d6b8a879e89e6ef3c8f0e89da99539f61e8763410c392d29f15d"),
    "bitswap": ("izi", 18811711, "b34a982f81682df1621b404095a9437d6d767ef12b3cf1a375fa8d81cf6c14b4"),
    "kyberswap-elastic-base": ("kyber", 2949023, "c26cb0a731d3fab0ed64a939b87bff8c9186b309e65b8f7f51a6eeb58647c162"),
    "kyberswap_elastic": ("kyber", 17291893, "c26cb0a731d3fab0ed64a939b87bff8c9186b309e65b8f7f51a6eeb58647c162"),
}
FEE_EVENT = "0x"+keccak(text="SwapFeeEnabled(uint24,int24)").hex()
POOL_EVENT = {k:"0x"+keccak(text=signature).hex() for k,signature in [
    ("izi","NewPool(address,address,uint24,uint24,address)"), ("kyber","PoolCreated(address,address,uint24,int24,address)")]}
GETTER = {"izi":"fee2pointDelta(uint24)", "kyber":"feeAmountTickDistance(uint24)"}


def calld(to, sig, suffix, tag):
    return "eth_call", [{"to":to, "data":"0x"+(keccak(text=sig)[:4]+suffix).hex()}, tag]


def spacing(value):
    return 0 if value == "0x" else decode(["int24"], bytes.fromhex(value[2:]))[0]


def market(log):
    x, y, fee = [int(t,16) for t in log["topics"][1:]]
    delta, pool = decode(["uint24","address"], bytes.fromhex(log["data"][2:]))
    if not 0 < x < y < 2**160 or not 0 <= fee < 2**24 or not delta or int(pool,16) == 0:
        raise ValueError("wrong market fields")
    return {"tokens":["0x"+x.to_bytes(20,"big").hex(), "0x"+y.to_bytes(20,"big").hex()],
            "fee":fee,"spacing":delta,"pool":pool,"block":int(log["blockNumber"],16),
            "block_hash":log["blockHash"],"tx":log["transactionHash"],"log_index":int(log["logIndex"],16)}


def markets(reader, profile, factory, kind, start, stop, final):
    initial = 1 if kind == "izi" else 3  # creation of both bytecode halves in BaseSplitCodeFactory
    if profile == "bitswap":
        prior = json.loads((FIXTURES/"bitswap_ethereum_registry.json").read_bytes())
        heights = sorted({m["block"] for m in prior["markets"]})
    else:
        heights = sorted(pool_blocks(reader,factory,start-1,stop,initial,final))
    ops = (("eth_getLogs",[{"address":factory,"fromBlock":hex(h),"toBlock":hex(h),"topics":[POOL_EVENT[kind]]}]) for h in heights)
    pools = []
    for h, logs in zip(heights,reader.many(ops),strict=True):
        for log in logs:
            if log["address"].lower()!=factory or log["topics"][0]!=POOL_EVENT[kind] or int(log["blockNumber"],16)!=h:
                raise ValueError("foreign market log")
            pools.append(market(log))
    if len(pools) != final-initial or len({m["pool"] for m in pools}) != len(pools):
        raise ValueError(("incomplete factory inventory",profile,len(pools),final))
    return pools


def history(reader, factory, kind, start, stop, tiers):
    # Обе фабрики единожды устанавливают конкретный шаг. У iZi это происходит без события.
    ranges = {f:(start-1,stop) for f in tiers}
    while any(b-a>1 for a,b in ranges.values()):
        mids = [(f,(a+b)//2) for f,(a,b) in ranges.items() if b-a>1]
        values = reader.many(calld(factory,GETTER[kind],word(f),hex(h)) for f,h in mids)
        for (f,h),raw in zip(mids,values,strict=True):
            a,b=ranges[f]; found=spacing(raw)
            if found not in (0,tiers[f]):
                raise ValueError(("mutated fee",factory,f,h))
            ranges[f]=(a,h) if found else (h,b)
    heights=sorted({h for _,h in ranges.values()})
    headers=dict(zip(heights,reader.many(("eth_getBlockByNumber",[hex(h),False]) for h in heights),strict=True))
    result=[]
    for f,(before,h) in ranges.items():
        b=headers[h]
        if int(b["number"],16)!=h or before+1!=h:
            raise ValueError("wrong activation height")
        args=[calld(factory,GETTER[kind],word(f),{"blockHash":key,"requireCanonical":True}) for key in (b["parentHash"],b["hash"])]
        pre,post=map(spacing,reader.many(args))
        if (pre,post)!=(0,tiers[f]):
            raise ValueError(("activation boundary",f,pre,post))
        row={"fee":f,"spacing":post,"block":h,"hash":b["hash"],"parent":b["parentHash"],"before":pre}
        if kind=="kyber":
            selector={"blockHash":b["hash"],"address":factory,"topics":[FEE_EVENT,"0x"+word(f).hex()]}
            logs=reader.one("eth_getLogs",[selector])
            if len(logs)!=1:
                raise ValueError(("missing SwapFeeEnabled",factory,f,h))
            log=logs[0]; receipt=reader.one("eth_getTransactionReceipt",[log["transactionHash"]])
            same=[x for x in receipt["logs"] if x["address"].lower()==factory and x["logIndex"]==log["logIndex"]]
            if receipt["blockHash"]!=b["hash"] or int(receipt["blockNumber"],16)!=h or len(same)!=1 or same[0]["topics"]!=[FEE_EVENT,"0x"+word(f).hex(),"0x"+word(post).hex()] or same[0]["data"]!="0x":
                raise ValueError("wrong original fee receipt")
            row.update(tx=receipt["transactionHash"],log_index=int(log["logIndex"],16))
        result.append(row)
    return result


def verify(data):
    factory,kind=data["factory"],data["kind"]
    proof=data["proof"]; root=prove_account({"stateRoot":data["state_root"]},proof,factory)
    if proof["codeHash"]!=data["code_hash"] or data["code_hash"]!="0x"+SPECS[data["id"]][2]:
        raise ValueError("wrong factory runtime")
    pools=data["markets"];expected_nonce=1 if kind=="izi" else 3
    if int(proof["nonce"],16)!=len(pools)+expected_nonce:
        raise ValueError("unexpected pool count")
    slots={i:int(value,16) for i,value in enumerate(data["slots"]) if int(value,16)}
    fee_slot,pool_slot=(3,2) if kind=="izi" else (6,7)
    tiers={int(f):int(s) for f,s in data["tiers"].items()}
    for f,s in tiers.items():
        if s<=0 or (kind=="kyber" and not(0<=f<100000 and s<16384)):
            raise ValueError("invalid positive tier")
        slots[map_slot(f,fee_slot)]=s
    for m in pools:
        if tiers[m["fee"]]!=m["spacing"]:
            raise ValueError("pool uses foreign spacing")
        x,y=[int(t,16) for t in m["tokens"]]
        for a,b in ((x,y),(y,x)):
            slots[map_slot(m["fee"],map_slot(b,map_slot(a,pool_slot)))]=int(m["pool"],16)
    if kind=="kyber":
        managers=data["nft_managers"]
        if slots.get(8,0)!=len(managers):
            raise ValueError("unknown whitelisted managers")
        first=int.from_bytes(keccak(word(8)),"big")
        for i,a in enumerate(managers):
            slots[first+i]=int(a,16)
            slots[map_slot(int(a,16),9)]=i+1
    got=keccak(rlp.encode(trie_node([(nibbles(keccak(word(k))),rlp.encode(v)) for k,v in slots.items()])))
    if got!=root:
        raise ValueError((data["id"],"unexplained storage; completeness cannot be claimed",len(pools),sorted(tiers)))


def record(network):
    head=json.loads((FIXTURES/(network+".json")).read_bytes())
    book=json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    reader=Reader(network,head);results=[]
    for p in (p for p in book if p["network"]==network and p["id"] in SPECS):
        id=p["id"];settings=p["settings"];factory=settings["dex.factory"].lower()
        kind,start,code=SPECS[id]
        bytecode=reader.one("eth_getCode",[factory,reader.tag])
        if keccak(hexstr=bytecode).hex()!=code:
            raise ValueError(("factory code differs",id))
        proof=reader.one("eth_getProof",[factory,[],reader.tag]);proof.pop("storageProof",None)
        final=int(proof["nonce"],16)
        pools=markets(reader,id,factory,kind,start,head["block"],final)
        candidates={int(f) for f in settings["dex.v3_fee_tiers"]}|{m["fee"] for m in pools}|{0,8,10,20,40,50,80,100,200,250,300,400,500,750,1000,2000,2500,3000,5000,10000}
        candidates=sorted(candidates)
        values=reader.many(calld(factory,GETTER[kind],word(f),reader.tag) for f in candidates)
        tiers={f:spacing(v) for f,v in zip(candidates,values,strict=True) if spacing(v)}
        slots=reader.many(("eth_getStorageAt",[factory,hex(i),reader.tag]) for i in range(15 if kind=="izi" else 10))
        row={"id":id,"kind":kind,"factory":factory,"code_hash":"0x"+code,"deployment":start,"state_root":reader.header["stateRoot"],
             "proof":proof,"tiers":tiers,"slots":slots,"markets":pools}
        if kind=="kyber":
            raw=reader.many([calld(factory,"getWhitelistedNFTManagers()",b"",reader.tag)])[0]
            row["nft_managers"]=list(decode(["address[]"],bytes.fromhex(raw[2:]))[0])
        verify(row)
        row["activations"]=history(reader,factory,kind,start,head["block"],tiers)
        results.append(row)
    if len(results)!=2:raise ValueError("expected both factory families")
    return {"chain_id":head["chain_id"],"block":head["block"],"selector":reader.tag,"factories":results}


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("network",choices=("base","ethereum"));cli.add_argument("file",type=Path)
    a=cli.parse_args()
    a.file.write_text(json.dumps(record(a.network),separators=(",",":")),encoding="utf8")
