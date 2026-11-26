"""Архивные VirtualQuoter и ArbExecutor V5 на всех подтверждённых CREATE2 фабрики DerpDEX Base.
Для каждого рынка — обе стороны в исходном блоке, 4 величины входа в единицах актива,
затем покупка и продажа наблюдаемого выхода изолированным плательщиком.
"""
import json,sys
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT,OWNER,RUNTIME,calldata,call
from record_pool_executor_controls import record,simulate_block
from record_metric_chains import VIRTUAL
from derpdex_archive_rpc import Readers
from record_derpdex_base import FACTORY,FOLDER,WETH
from record_rubicon_clmm import pool_address


USDBC="0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca"
REGULAR_GAS=1_200_000
UPPER_GAS=16_777_216

def quote_result(reply, requested, gas_limit):
    value={"status": reply["status"], "returnData": reply.get("returnData"), "gas_limit":gas_limit,
           "gas_used":int(reply["gasUsed"],16)}
    if int(reply["status"],16)==1:
        used,received=decode(["uint256","uint256"],bytes.fromhex(reply["returnData"][2:]))
        if used>requested:raise ValueError("пул сообщил сверх запрошенного входа")
        value.update(used=str(used),out=str(received),full=(used==requested and received>0))
    else:
        value["reason"]=reply.get("error")
    return value
def main(out):
    directory=json.loads((FOLDER/"derpdex_directory.json").read_bytes())
    old=json.loads((FOLDER/"derpdex_base.json").read_bytes())
    tag=directory["selector"]; init=bytes.fromhex(old["provenance"]["pool"]["actual"]["init_hash"][2:])
    virtual=(ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x")
    reference=old["execution"]["pools"][0]
    case=reference["cases"][0]
    initial=int(case["input"])
    probe=call(VIRTUAL,calldata("quoteExactIn(address,bool,uint256)",["address","bool","uint256"],
                                [reference["pool"],case["side"]==0,initial]))
    expected="0x"+initial.to_bytes(32,"big").hex()+int(case["bought"]).to_bytes(32,"big").hex()
    nodes=Readers(tag,directory["to"],directory["state_root"],FACTORY,{VIRTUAL:{"code":"0x"+virtual}},probe,expected)
    url=nodes.urls[0]
    def read(a,s,types=(),args=()):
        raw=nodes.read(url,"eth_call",[{"to":a,"data":calldata(s,types,args)},tag])
        return bytes.fromhex(raw[2:])
    def num(a,s):return int.from_bytes(read(a,s),"big")
    tokens={a for p in directory["pools"] for a in p["tokens"]}
    decimals={a:num(a,"decimals()") for a in tokens}
    def sizes(a):
        if a==WETH:return [10**12,10**14,10**16,10**17]
        unit=10**decimals[a]
        return [max(1,unit//1000),max(1,unit//10),unit*10,unit*100]
    def static(p):
        ts=p["tokens"];fee=p["fee"];addr=p["pool"]
        assert pool_address(FACTORY,ts,fee,init)==addr
        for path in [ts,list(reversed(ts))]:
            assert decode(["address"],read(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[*path,fee]))[0]==addr
        actual=[decode(["address"],read(addr,s))[0] for s in ["token0()","token1()","factory()"]]
        if actual!=[*ts,FACTORY] or num(addr,"fee()")!=fee or num(addr,"tickSpacing()")!=p["spacing"]:
            raise ValueError("поля не прошли проверку: "+addr)
        plan=[(i,amount) for i,a in enumerate(ts) for amount in sizes(a)]
        probes=[call(VIRTUAL,calldata("quoteExactIn(address,bool,uint256)",["address","bool","uint256"],[addr,i==0,n])) for i,n in plan]
        overrides={VIRTUAL:{"code":"0x"+virtual}}
        resp=simulate_block(url,tag,overrides,probes,read_rpc=nodes.read)
        quotes=[]
        for (side,n),probe,r in zip(plan,probes,resp,strict=True):
            q={"side":side,"requested":str(n),**quote_result(r,n,REGULAR_GAS)}
            if "reason" in q:
                widened={**probe,"gas":hex(UPPER_GAS)}
                test=simulate_block(url,tag,overrides,[widened],read_rpc=nodes.read)
                q["larger_gas"]=quote_result(test[0],n,UPPER_GAS)
            quotes.append(q)
        return {**p,"liquidity":str(num(addr,"liquidity()")),"quotes":quotes}
    with ThreadPoolExecutor(max_workers=2) as pool:
        markets=list(pool.map(static,directory["pools"]))
    bridges={}
    for m in markets:
        if WETH not in m["tokens"]:continue
        asset=next(a for a in m["tokens"] if a!=WETH)
        if asset not in bridges or int(m["liquidity"])>int(bridges[asset]["liquidity"]):bridges[asset]=m
    def wallet(item):
        index,m=item;ts=m["tokens"]
        side=ts.index(WETH) if WETH in ts else ts.index(USDBC) if USDBC in ts else next((i for i,a in enumerate(ts) if a in bridges),0)
        funding=bridges.get(ts[side],{}) if ts[side]!=WETH else {}
        prepared=dict(idx=index,pool=m["pool"],tokens=ts,quotes=[{"side":side,"input":str(x)} for x in sizes(ts[side])],
                      funding_weth=10**18//4,funding_pool=funding.get("pool"))
        source=dict(kind="v3",network="base",endpoint=url,read_rpc=nodes.read,hash=tag["blockHash"],block=directory["to"],contracts={"factory":FACTORY},
                    quoter_runtime_keccak="0x"+keccak(bytes.fromhex(virtual)).hex(),capture_partial=True,pools=[prepared])
        replay=record(source)
        m["funding_pool"]=prepared["funding_pool"];m["wallet"]=replay["pools"][0]["cases"]
        remaining=[{"side":r["side"],"input":r["input"]} for r in m["wallet"] if "sold" not in r and any(
            q["side"]==r["side"] and q["requested"]==r["input"] and int(q.get("larger_gas",{}).get("out",0))>0 for q in m["quotes"])]
        if remaining:
            second=record({**source,"target_gas":UPPER_GAS,"pools":[{**prepared,"quotes":remaining}]})
            m["larger_gas_wallet"]=second["pools"][0]["cases"]
        if not any("sold" in r for r in m["wallet"]) and any(int(q.get("out",0))>0 for q in m["quotes"]):
            creation=nodes.read(url,"eth_getTransactionByHash",[m["tx"]])
            if creation["hash"]!=m["tx"] or creation["blockHash"]!=m["hash"]:
                raise ValueError("неподтверждённое происхождение отправителя")
            holder=creation["from"]
            if nodes.read(url,"eth_getCode",[holder,tag])=="0x":
                balances=[int.from_bytes(read(a,"balanceOf(address)",["address"],[holder]),"big") for a in ts]
                eligible=[{"side":q["side"],"input":q["requested"]} for q in m["quotes"]
                          if int(q.get("out",0))>0 and int(q["requested"])<=balances[q["side"]]]
                if eligible:
                    seeded=record({**source,"use_owner_tokens":True,"pools":[{**prepared,"quotes":eligible}]},owner=holder)
                    m["existing_holder"]={"sender":holder,"creation_tx":m["tx"],"balances":list(map(str,balances)),
                                          "wallet":seeded["pools"][0]["cases"]}
        return m
    with ThreadPoolExecutor(max_workers=2) as pool:
        markets=list(pool.map(wallet,enumerate(markets)))
    result={"chain_id":8453,"selector":tag,"height":directory["to"],"endpoint":url,"sender":OWNER,
            "factory":FACTORY,"decimals":decimals,"markets":markets,"index":[x["pool"] for x in directory["indexed"]],
            "verified_endpoints":nodes.counts,"state_root":directory["state_root"],"virtual_quoter_keccak":"0x"+keccak(bytes.fromhex(virtual)).hex(),
            "executor_runtime_keccak":"0x"+keccak(bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))).hex()}
    if len(markets)!=13 or len(result["index"])!=6 or not all(len(m["quotes"])==8 and len(m["wallet"])==4 for m in markets):
        raise ValueError("неполная таблица рынков/размеров")
    reference=next(m for m in markets if m["pool"]==old["provenance"]["pool"]["address"])
    if not all("sold" in row for row in reference["wallet"]):raise ValueError("прежде исполненный рынок не воспроизведён")

    Path(out).write_bytes((json.dumps(result,indent=2)+"\n").encode())

if __name__=="__main__":main(sys.argv[1])
