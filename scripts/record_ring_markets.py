"""Проверяет канонические пары из полного allPairs Ring покупками на архивном Ethereum.

Кошелёк получает только виртуальный ETH. Другой underlying покупается на отдельной
канонической паре Ring WETH, если на блоке хватает реального объёма. Сохраняет
каждую попытку и точную причину ограничения, включая непригодную пару пополнения.
"""
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import requests
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, calldata, call, rpc
from record_ring_ethereum import BALANCE, QUOTER, WETH, ROOT, allow, exchange
from record_v2_chain_pools import minimum

def measured(endpoint, tag, deadline, group, row, bridges, tier):
    tokens=group["tokens"]
    physical=row["wrappers"]
    assets=[tokens[w]["asset"] for w in physical]
    reserves=list(map(int,row["reserves"]))
    cap,divisor=[(10**15,10000),(10**16,500)][tier]
    offers=[]
    for i,asset in enumerate(assets):
        ri,ro=reserves[i],reserves[1-i]
        limit=ri//divisor
        bridge=None
        if asset==WETH:
            cost=amount=min(cap,limit)
            funded=amount
        elif asset in bridges:
            bridge=bridges[asset]
            if bridge["pool"]==row["pool"]:continue
            w0,w1=bridge["wrappers"]
            r0,r1=map(int,bridge["reserves"])
            if tokens[w0]["asset"]!=WETH:
                w0,w1=w1,w0
                r0,r1=r1,r0
            if not r0 or not r1 or not limit:continue
            target=min(limit, minimum(cap,r0,r1,3000))
            if not target or target>=r1:continue
            cost=(target*r0*1000+(r1-target)*997-1)//((r1-target)*997)
            if not 0<cost<=cap:continue
            funded=minimum(cost,r0,r1,3000)
            amount=min(limit,funded)
            bridge=(w0,w1)
        else:continue
        bought=minimum(amount,ri,ro,3000) if amount and ri else 0
        returned=minimum(bought,ro-bought,ri+amount,3000) if bought else 0
        if min(amount,bought,returned)>0:
            offers.append((bridge is None,cost,i,bridge,amount,funded,bought,returned))
    if not offers:return {"tier":tier,"status":"no_funded_positive_size"}
    _,cost,i,bridge,amount,funded,bought,returned=max(offers,key=lambda x:(x[0],x[1]))
    tin,tout=assets[i],assets[1-i]; w_in,w_out=physical[i],physical[1-i]
    result={"tier":tier,"input":tin,"output":tout,"weth_cost":str(cost),"funded":str(funded),
            "input_raw":str(amount),"out_raw":str(bought),"return_raw":str(returned),"bridge":bridge}
    if any(int(tokens[w]["backing"] or 0)<out for w,out in [(w_out,bought),(w_in,returned)]):
        # Вход увеличит backing первой стороны. Сохраняем саму симуляцию: выражение здесь лишь предупреждает о состоянии до неё.
        result["small_initial_backing"]=True
    steps=[]
    def add(label,tx):steps.append((label,tx))
    def approve(asset,quantity):
        for a,tx in enumerate(allow(asset,quantity)):add("approve_"+asset[-5:]+str(a),tx)
    def bal(label,token): add(label,call(token,calldata(BALANCE,["address"],[OWNER])))
    add("deposit",call(WETH,calldata("deposit()"),cost))
    if bridge:
        approve(WETH,cost)
        add("fund",exchange(list(bridge),tin,cost,funded,deadline,input_asset=WETH))
    approve(tin,amount);bal("start",tin);bal("output_at_start",tout)
    for d,(src,dst,wp,qty,out,prior) in enumerate([(tin,tout,[w_in,w_out],amount,bought,0),
                                                  (tout,tin,[w_out,w_in],bought,returned,funded-amount)]):
        if d:approve(src,qty)
        add("quote"+str(d),call(QUOTER,calldata("getAmountsOut(uint256,address[])",["uint256","address[]"],[qty,wp])))
        add("limit"+str(d),exchange(wp,dst,qty,out+1,deadline,check=prior+out,input_asset=src))
        add("balance_limit"+str(d),exchange(wp,dst,qty,out,deadline,check=prior+out+1,input_asset=src))
        add("swap"+str(d),exchange(wp,dst,qty,out,deadline,check=prior+out,input_asset=src))
        bal("after_dst"+str(d),dst);bal("after_src"+str(d),src)
    payload=[{"blockStateCalls":[{"stateOverrides":{OWNER:{"balance":hex(10**19)}},"calls":[tx for _,tx in steps]}]},tag]
    for attempt in range(3):
        try:
            calls=rpc(endpoint,"eth_simulateV1",payload)[0]["calls"]
            break
        except (requests.RequestException, ValueError, KeyError) as err:
            if attempt==2:return {**result,"status":"rpc_unavailable","reason":str(err)[:240]}
            time.sleep(1+attempt)
    got={name:r for (name,tx),r in zip(steps,calls,strict=True)}
    evidence={}
    for (name,tx),r in zip(steps,calls,strict=True):
        fail=name.startswith(("limit","balance_limit"))
        ok=int(r["status"],16)>0
        if name.startswith(("swap","limit","balance_limit","fund")) or not ok:
            evidence[name]={"status":r["status"],"gas":int(r["gasUsed"],16),
                            "calldata_keccak":"0x"+keccak(bytes.fromhex(tx["data"][2:])).hex(),
                            **({"error":r.get("error")} if not ok else {})}
        if fail==ok:
            return {**result,"status":"control_mismatch" if fail else "reverted","at":name,"evidence":evidence}
    def val(key):return int(got[key]["returnData"],16)
    observed=[val(n) for n in ["start","output_at_start","after_dst0","after_src0","after_dst1","after_src1"]]
    expected=[funded,0,bought,funded-amount,funded-amount+returned,0]
    quotes=[list(decode(["uint256[]"],bytes.fromhex(got["quote"+str(d)]["returnData"][2:]))[0]) for d in range(2)]
    if observed!=expected or quotes!=[[amount,bought],[bought,returned]]:
        return {**result,"status":"different_payer_delta_or_quote","observed":list(map(str,observed)),"evidence":evidence}
    return {**result,"status":"both_swaps","observed":list(map(str,observed)),"evidence":evidence}

def main(out):
    directory=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ring_directory.json").read_bytes())
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    url,tag=snap["endpoint"],snap["selector"]
    if tag!=directory["selector"] or int(rpc(url,"eth_chainId",[]),16)!=1:raise ValueError("сеть/блок расходятся")
    header=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False]);deadline=int(header["timestamp"],16)+120
    rows=[r for r in directory["rows"] if r["verified"]]
    bridges={}
    for row in rows:
        a,b=[directory["tokens"][w]["asset"] for w in row["wrappers"]]
        if a==WETH:bridges[b]=row
        elif b==WETH:bridges[a]=row
    def check(row):
        cases=[]
        for tier in range(2):
            cases.append(measured(url,tag,deadline,directory,row,bridges,tier))
        return {"pool":row["pool"],"index":row["index"],"cases":cases}
    with ThreadPoolExecutor(max_workers=3) as executor:
        markets=list(executor.map(check,rows))
    value={"chain_id":1,"selector":tag,"registered":directory["count"],"markets":markets}
    Path(out).write_bytes((json.dumps(value,indent=2)+"\n").encode())
    # Проверяем новые крупные пары, где ни один underlying не WETH.
    required={23,24}
    for r in markets:
        if r["index"] in required and all(c["status"]=="both_swaps" for c in r["cases"]):
            required.remove(r["index"])
    if required:raise ValueError("Не прошли обе стороны проверочных рынков WBTC/USDC и WBTC/USDT: "+str(sorted(required)))

if __name__=="__main__":
    main(sys.argv[1])
