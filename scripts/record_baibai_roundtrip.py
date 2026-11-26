"""Возврат полученных NVDAc тем же кошельком и в том же состоянии после покупки; прямой и модульный пути."""
from eth_abi import decode
from record_baibai import ENTRY, FOLDER, USDC, Capture, abi, available, read, simulate as native, write
from record_baibai_nvdac import COIN
from record_elfomo_executor import EXEC, ORDER, TYPES, calldata, simulate as module


def capture(code=None):
    source=available(read(FOLDER/"baibai_nvdac_base.json"))
    c=Capture(source)
    wallet=source["cases"][0]["owner"]
    target=EXEC if code else ENTRY
    deadline=int(c.one("eth_getBlockByHash",[c.tag["blockHash"],False])["timestamp"],16)+90
    addr,num="address","uint256"
    def run(steps):
        if code:
            tx, r=module(source,code,wallet,steps)
            return dict(calls=tx,results=r)
        return native(c,wallet,steps)
    def quote(a,b,n):
        return ((EXEC, abi("quoteBai",[addr,addr,num],[a,b,n])) if code else (
            ENTRY,abi("quoteFor",[addr,addr,num,addr],[COIN,a,n,wallet])))
    def swap(a,b,n,minimum):
        return ((EXEC,calldata(ORDER,TYPES,[ENTRY,a,b,n,minimum,deadline])) if code else (
            ENTRY,abi("swapExactAmountIn",[addr,addr,num,num,addr],[COIN,a,n,minimum,wallet])))
    def approve(token,n):
        return (token,abi("approve",[addr,num],[target,n]))
    def balances(suffix):
        return [(key+suffix,token,abi("balanceOf",[addr],[wallet])) for key,token in (("usd",USDC),("coin",COIN))]
    def output(r,key):
        step=r["results"][key]
        if step["status"]!="0x1":
            raise ValueError(f"{key}: {step.get('error')}")
        return decode(["uint256"],bytes.fromhex(step["returnData"][2:]))[0]
    amount=10**7
    buy=[("q_buy",*quote(USDC,COIN,amount)),("reset_usd",*approve(USDC,0)),("approve_usd",*approve(USDC,amount))]
    purchased=output(run(buy),"q_buy")
    first=balances("_before")+buy+[("buy",*swap(USDC,COIN,amount,purchased))]+balances("_middle")
    first += [("q_sell",*quote(COIN,USDC,purchased)),("reset_coin",*approve(COIN,0)),("approve_coin",*approve(COIN,purchased))]
    floor=output(run(first),"q_sell")
    all_steps=first+[("above_sell",*swap(COIN,USDC,purchased,floor+1)),("sell",*swap(COIN,USDC,purchased,floor))]+balances("_after")
    sample=run(all_steps)
    for key,row in sample["results"].items():
        if row["status"] != ("0x0" if key=="above_sell" else "0x1"):
            raise ValueError(f"{key}: {row.get('error')}")
    n=lambda k:output(sample,k)
    if ((n("usd_before")-n("usd_middle"),n("coin_middle")-n("coin_before"),n("coin_middle")-n("coin_after"),
         n("usd_after")-n("usd_middle"),n("q_buy"),n("q_sell"),n("buy"),n("sell"))
            != (amount,purchased,purchased,floor,purchased,floor,purchased,floor)):
        raise ValueError("Дельты кошелька и пересчитанный на состоянии после покупки выход расходятся")
    return dict(chain_id=8453,selector=c.tag,endpoint=c.url,wallet=wallet,called=target,base=COIN,quote=USDC,
                spent=str(amount),purchased=str(purchased),returned=str(floor),**sample)


if __name__=="__main__":
    write(FOLDER/"baibai_nvdac_roundtrip.json",capture())
