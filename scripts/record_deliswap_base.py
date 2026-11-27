"""Архив выплат DeliHookConstantProduct через Universal Router и V5. Только симуляция."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, RUNTIME, EXEC, STEP, calldata, rpc
from record_diamondswap_base import WETH
from record_omni_v2_base import ENDPOINTS, role
from record_solidly_v3 import HOLDERS

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
KEY = "(address,address,uint24,int24,address)"
WBLT = "0x4e74d4db6c0726ccded4656d0bce448876bb4c7a"
BMX = "0x548f93779fbc992010c07467cbaf329dd5f059b7"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
HOOK = "0x95afbc0fccf974b41380f24e562f15b6dd90fac8"
AERO = "0xcf77a3ba9a5ca399b7c97c74d54e5b1beb874e43"
FACTORY = "0x420dd381b31aef6683db6b902084cb0ffece40da"
PAYER = HOLDERS["base"][USDC]
ROUTES = "(address,address,bool,address)[]"


def tx(to, signature, types=(), args=()):
    return {"from": PAYER, "to": to, "data": calldata(signature, types, args), "gas": "0x900000"}


def quantity(result):
    if result["status"] != "0x1":
        raise ValueError(str(result.get("error"))[:220])
    return int(result["returnData"], 16)


def capture(endpoint):
    snap = json.loads((DATA / "base.json").read_bytes())
    c = Capture(dict(snap, endpoint=endpoint))
    head = rpc(endpoint, "eth_getBlockByHash", [c.selector["blockHash"], False])
    profiles = json.loads((ROOT / "config/first_run_dex.json").read_bytes())
    cfg = next(p["settings"] for p in profiles if p["id"] == "uniswap-v4-base")
    mgr, ur, permit = [cfg[k].lower() for k in ("dex.pool_manager", "dex.universal_router", "dex.permit2")]
    deadline = int(head["timestamp"], 16) + 180
    if rpc(endpoint, "eth_getCode", [PAYER, c.selector]) != "0x" or rpc(endpoint, "eth_getCode", [EXEC,c.selector]) != "0x":
        raise ValueError("исходный держатель либо адрес изолированного исполнителя имеет код")
    _, hook_code = role(c, HOOK)
    roles = {k: c.one(HOOK, k+"()", returns=["address"])[0] for k in
             ("poolManager", "WBLT", "BMX", "feeProcessor", "incentiveGauge", "dailyEpochGauge")}
    if (roles["poolManager"], roles["WBLT"], roles["BMX"]) != (mgr,WBLT,BMX):
        raise ValueError("размещение Deli изменилось")
    hashes = {}
    for addr in {ur, permit, mgr, *roles.values()}:
        code = bytes.fromhex(rpc(endpoint, "eth_getCode", [addr, c.selector])[2:])
        if not code: raise ValueError("в указанной роли нет кода")
        hashes[addr] = "0x"+keccak(code).hex()
    if c.one(AERO,"defaultFactory()",returns=["address"]) != (FACTORY,):
        raise ValueError("подготовительный маршрут Aero сменил фабрику")
    routes = [(USDC, BMX, False, FACTORY), (BMX, WBLT, False, FACTORY)]
    amount = 10**6
    initial = c.one(USDC,"balanceOf(address)",["address"],[PAYER],["uint256"])[0]
    if initial < amount: raise ValueError("нет собственных USDC для подготовки")
    quoted, = c.one(AERO,f"getAmountsOut(uint256,{ROUTES})",["uint256",ROUTES],[amount,routes],["uint256[]"])
    if quoted[-1] < 10**13: raise ValueError("недостаточная подготовительная котировка")
    funding = [tx(USDC,"approve(address,uint256)",["address","uint256"],[AERO,amount]),
               tx(AERO,f"swapExactTokensForTokens(uint256,uint256,{ROUTES},address,uint256)",
                  ["uint256","uint256",ROUTES,"address","uint256"],[amount,quoted[-1],routes,PAYER,deadline])]
    def simulate(calls, mode):
        frame = {"calls": calls}
        if mode == "v5":
            word = lambda n: "0x"+format(n,"064x")
            frame["stateOverrides"] = {EXEC: {"code": "0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                                               "stateDiff": {word(0): word(int(PAYER,16)),word(1): word(int(WETH,16))}}}
        block, = rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},c.selector])
        if block["parentHash"] != c.selector["blockHash"]: raise ValueError("смена родительского блока")
        return block["calls"]
    def leg(prefix,key,mode,x,y,size):
        zero = key[0] == x
        if mode == "v5":
            approvals = [tx(x,"approve(address,uint256)",["address","uint256"],[EXEC,size])]
            def order(minimum,end):
                return tx(EXEC,f"execute(address,uint256,address,uint256,uint256,{STEP}[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],
                          [x,size,y,minimum,end,[(19,mgr,x,y,10000,encode([KEY],[key]))]])
        else:
            approvals = [tx(x,"approve(address,uint256)",["address","uint256"],[permit,size]),
                         tx(permit,"approve(address,address,uint160,uint48)",["address","address","uint160","uint48"],[x,ur,size,deadline])]
            def order(minimum,end):
                swap = encode([f"({KEY},bool,uint128,uint128,uint256,bytes)"],[(key,zero,size,minimum,0,b"")])
                settle = encode(["address","uint256"],[x,size])
                take = encode(["address","uint256"],[y,minimum])
                commands = encode(["bytes","bytes[]"],[bytes([6,12,15]),[swap,settle,take]])
                return tx(ur,"execute(bytes,bytes[],uint256)",["bytes","bytes[]","uint256"],[bytes([16]),[commands],end])
        bal = [tx(t,"balanceOf(address)",["address"],[PAYER]) for t in (x,y)]
        quoted = tx(HOOK,f"getAmountOut({KEY},bool,uint256)",[KEY,"bool","uint256"],[key,zero,size])
        pid = keccak(encode([KEY],[key]))
        reserves = tx(HOOK,"getReserves(bytes32)",["bytes32"],[pid])
        pre = prefix+approvals+bal+[reserves, quoted]
        front = simulate(pre,mode)
        if any(r["status"]!="0x1" for r in front): raise ValueError(("setup",[(i,r.get("error",{}).get("message")) for i,r in enumerate(front) if r["status"]!="0x1"]))
        out = quantity(front[-1])
        rin,rout = decode(["uint256","uint256"],bytes.fromhex(front[-2]["returnData"][2:]))
        if not zero: rin,rout=rout,rin
        effective=size*(10**6-key[2])
        if not out or out != effective*rout//(10**6*rin+effective):raise ValueError("котировка расходится с резервами hook")
        # Каждый контроль начинается при той же высоте и на тех же резервах; откат не меняет состояние.
        calls = pre+[order(out+1,deadline),order(out,0),order(out,deadline)]+bal+[reserves]
        records = simulate(calls,mode)
        if [v["status"] for v in records[-6:-3]] != ["0x0","0x0","0x1"]:
            raise ValueError((mode, x, size, [v.get("error",{}).get("message",v["status"]) for v in records[-6:-3]]))
        if quantity(front[-4])-quantity(records[-3])!=size or quantity(records[-2])-quantity(front[-3])!=out:
            raise ValueError("расхождение выплаты с балансом кошелька")
        return prefix+approvals+[order(out,deadline)],out,dict(input=str(size),output=str(out),calls=calls,results=records)
    markets=[]
    for init in json.loads((DATA/"deli_base_initializations.json").read_bytes()):
        fee,spacing,h,_,_=decode(["uint24","int24","address","uint160","int24"],bytes.fromhex(init["data"][2:]))
        key=["0x"+t[-40:] for t in init["topics"][2:4]]+[fee,spacing,h]
        pid="0x"+keccak(encode([KEY],[key])).hex()
        if init["address"]!=mgr or pid!=init["topics"][1] or h!=HOOK or WBLT not in key[:2]:raise ValueError("чужой PoolKey")
        birth=rpc(endpoint,"eth_getBlockByHash",[init["blockHash"],False])
        if birth["number"]!=init["blockNumber"] or int(birth["number"],16)>int(head["number"],16):raise ValueError("поздняя запись")
        other=next(t for t in key[:2] if t!=WBLT)
        balances = c.one(HOOK,"getReserves(bytes32)",["bytes32"],[bytes.fromhex(pid[2:])],["uint128","uint128"])
        liquid = balances[key[:2].index(WBLT)]
        sizes = sorted({min(10**12,max(1,liquid//1000)), min(10**13,max(2,liquid//100))})
        cases=[]
        for size in sizes:
            for mode in ("router","v5"):
                case=dict(mode=mode,input=str(size))
                try:
                    next_calls,got,case["buy"]=leg(funding,key,mode,WBLT,other,size)
                    _,_,case["sell"]=leg(next_calls,key,mode,other,WBLT,got)
                except ValueError as e:
                    case["rejected"]=str(e)[:450]
                cases.append(case)
        markets.append(dict(pool_id=pid,key=key,reserves=list(map(str,balances)),cases=cases))
    if not any(all("sell" in case for case in row["cases"]) for row in markets):
        raise ValueError(str([m["cases"][0].get("rejected") for m in markets])[:900])
    return dict(selector=c.selector,owner=PAYER,initial_usdc=str(initial),funding=funding,hook=hook_code,roles=roles,
                runtime_hashes=hashes,manager=mgr,router=ur,permit2=permit,markets=markets)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    for endpoint in ENDPOINTS:
        try:
            data=capture(endpoint)
            args.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8",newline="\n")
            break
        except (ValueError,KeyError,OSError) as error:
            print(endpoint,str(error)[:500],flush=True)
    else:
        raise ValueError("Архив Deli не подтверждён")
