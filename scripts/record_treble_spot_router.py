"""Сравнивает фактическую выплату штатного Treble Spot SwapRouter со штатным шагом Algebra V5."""
from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import rpc

ROUTER="0x347539d82a14c0a3a89769f1024774a545a151ac"
ZERO="0x"+"00"*20
PARAMS="(address,address,address,address,uint256,uint256,uint256,uint160)"

def data(name,types,args):
    return "0x"+(keccak(text=name)[:4]+encode(types,args)).hex()

def record(endpoint,tag,header,capture,router):
    owner=capture["owner"];source=capture["token_in"];target=capture["calls"][1]["to"]
    qty=int(capture["amount"]);bought=int(capture["bought"]);sold=int(capture["sold"])
    now=int(header["timestamp"],16)
    def action(to,calldata):
        return {"from":owner,"to":to,"data":calldata,"gas":hex(1200000)}
    def balance(token,holder):
        return action(token,data("balanceOf(address)",["address"],[holder]))
    def allowance(token):
        return action(token,data("allowance(address,address)",["address"]*2,[owner,router]))
    def approve(token,n):
        return action(token,data("approve(address,uint256)",["address","uint256"],[router,n]))
    def swap(a,b,n,floor,when):
        return action(router,data("exactInputSingle("+PARAMS+")",[PARAMS],[(a,b,ZERO,owner,when,n,floor,0)]))
    calls=[("before_source",balance(source,owner)),("before_target",balance(target,owner))]
    for side,a,b,n,floor in [("buy",source,target,qty,bought),("sell",target,source,bought,sold)]:
        calls.extend([(side+"_approve",approve(a,n)),(side+"_plus_one",swap(a,b,n,floor+1,now+120)),
                      (side+"_expired",swap(a,b,n,floor,now-1)),(side,swap(a,b,n,floor,now+120)),
                      (side+"_input",balance(a,owner)),(side+"_output",balance(b,owner)),(side+"_allowance",allowance(a))])
    frame,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[{"calls":[v for _,v in calls]}],"validation":False},tag])
    if frame["parentHash"]!=tag["blockHash"]:raise ValueError("изменился родительский блок")
    rows=dict(zip([k for k,_ in calls],frame["calls"],strict=True))
    for k,v in rows.items():
        wanted="0x0" if k.endswith(("_plus_one","_expired")) else "0x1"
        if v["status"]!=wanted:raise ValueError((router,k,v.get("error")))
    raw=lambda k:int(rows[k]["returnData"],16)
    if (raw("before_source")-raw("buy_input"),raw("buy_output")-raw("before_target"),
        raw("buy_output")-raw("sell_input"),raw("sell_output")-raw("buy_input"))!=(qty,bought,bought,sold):
        raise ValueError("обе денежные дельты Router/V5 расходятся")
    if raw("buy_allowance") or raw("sell_allowance") or (raw("buy"),raw("sell"))!=(bought,sold):
        raise ValueError("остаток разрешения или номинальная выплата расходятся")
    return dict(owner=owner,router=router,amount=str(qty),bought=str(bought),sold=str(sold),
                calls=[v for _,v in calls],results=rows)

