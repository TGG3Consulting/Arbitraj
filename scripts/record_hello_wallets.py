"""Сверяет реальную выплату HELLO V3 через штатные Router, Router02 и ArbExecutor. Только чтение."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, ROOT, RUNTIME, STEP, calldata, rpc
from record_sectorone_directory import BASE_ENDPOINTS, ETH_ENDPOINTS
from record_solidly_v3 import BASE_WETH, ETH_WETH, HOLDERS

DATA=ROOT/"tests/data/first_run_dex_2026-11-26"
# Периферия установлена по CREATE отправителя фабрики; далее проверяем роли для каждого адреса.
ROLES={
    "base":["0x23f51850711acce31f6b0b6fbba8a5eebfc205db","0x8f63a6c8656c2113160c57a35e934e557c53aedd",
            "0xe1210b50d8c4b880adcb8a32ee70a181229aacf6","0x76f80c455833d8c5096c295e0b525525197b4fae"],
    "ethereum":["0x4a9661f0c80afa93d16a6adb617fa239c1e8fb37","0x83a7afae53dcb8615879f5b7e348f2bbbdca25d2",
                "0x88b0adeb37ed1ad06bc67faeea02d6a76338a0e6","0xdfb2b114a251fbe255788b178692fbae8baa4566"],
}
QPARAM="(address,address,uint256,uint24,uint160)"

def capture(directory, endpoint):
    chain=directory["chain"]; tag=directory["selector"]; weth=BASE_WETH if chain=="base" else ETH_WETH
    head=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if head["stateRoot"]!=directory["state_root"]:raise ValueError("другой stateRoot")
    deadline=int(head["timestamp"],16)+300
    first,second,q1,q2=ROLES[chain]; contracts={}
    def tx(payer,to,sig,types=(),args=()):
        return {"from":payer,"to":to,"data":calldata(sig,types,args),"gas":"0x900000"}
    def run(payer,calls,mode):
        frame={"calls":calls}
        if mode=="v5":
            word=lambda n:"0x"+format(n,"064x")
            frame["stateOverrides"]={EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                                           "stateDiff":{word(0):word(int(payer,16)),word(1):word(int(weth,16))}}}
        block,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},tag])
        if block["parentHash"]!=tag["blockHash"] or len(block["calls"])!=len(calls):raise ValueError("сменилась высота")
        return block["calls"]
    ret=lambda r,t:decode(t,bytes.fromhex(r["returnData"][2:]))
    for addr in ROLES[chain]:
        raw=bytes.fromhex(rpc(endpoint,"eth_getCode",[addr,tag])[2:])
        calls=[tx(EXEC,addr,m+"()") for m in ("factory","deployer","WETH9")]
        rs=run(EXEC,calls,"native")
        if [ret(v,["address"])[0] for v in rs]!=[directory["factory"],directory["deployer"],weth]:raise ValueError("роль периферии")
        contracts[addr]=dict(bytes=len(raw),keccak256="0x"+keccak(raw).hex(),calls=calls,results=rs)
    treasury,=ret(run(EXEC,[tx(EXEC,directory["factory"],"protocolFeeRecipient()")],"native")[0],["address"])
    def leg(row,payer,prefix,mode,x,y,qty):
        spender={"router":first,"router02":second,"v5":EXEC}[mode]
        approval=lambda n:tx(payer,x,"approve(address,uint256)",["address","uint256"],[spender,n])
        balance=lambda t,who:tx(payer,t,"balanceOf(address)",["address"],[who])
        balances=[balance(t,who) for who in (payer,EXEC) for t in (x,y)]
        quotes=[tx(payer,q,"quoteExactInputSingle("+QPARAM+")",[QPARAM],[(x,y,qty,row["fee"],0)]) for q in (q1,q2)]
        pre=prefix+[approval(0),approval(qty),*balances,*quotes]
        rs=run(payer,pre,mode)
        if any(v["status"]!="0x1" for v in rs):
            return None,dict(reason="quote/prepare reverted",input=str(qty),calls=pre,results=rs)
        a,b=[ret(r,["uint256","uint160","uint32","uint256"])[0] for r in rs[-2:]]
        if a!=b:raise ValueError("два подтверждённых квотера дали разные выходы")
        if not a:return None,dict(reason="zero output",input=str(qty),calls=pre,results=rs)
        def order(minimum,expiry):
            if mode=="v5":
                return tx(payer,spender,f"execute(address,uint256,address,uint256,uint256,{STEP}[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],[x,qty,y,minimum,expiry,[(1,row["pool"],x,y,10000,b"")]])
            types=["address","address","uint24","address"]
            args=[x,y,row["fee"],payer]
            if mode=="router":
                types.append("uint256");args.append(expiry)
            arg="("+",".join(types+["uint256","uint256","uint160"])+")"
            single=tx(payer,spender,"exactInputSingle("+arg+")",[arg],[tuple(args+[qty,minimum,0])])
            if mode=="router":return single
            return tx(payer,spender,"multicall(uint256,bytes[])",["uint256","bytes[]"],[expiry,[bytes.fromhex(single["data"][2:])]])
        exact=order(a,deadline)
        calls=pre+[order(a+1,deadline),order(a,0),exact,*balances]
        results=run(payer,calls,mode);strict,expired,fill=results[-7:-4]
        if strict["status"]!="0x0" or expired["status"]!="0x0":raise ValueError("minOut+1 или истекший deadline пропущены")
        if fill["status"]!="0x1":
            return None,dict(reason="payment reverted",input=str(qty),quote=str(a),calls=calls,results=results)
        before=[int(r["returnData"],16) for r in rs[-6:-2]]
        after=[int(r["returnData"],16) for r in results[-4:]]
        used=before[0]-after[0];net=after[1]-before[1]
        if not 0<used<=qty or net!=a or after[2:]!=before[2:]:raise ValueError("несовпадение конечного остатка или выплаты")
        data=dict(input=str(qty),paid=str(used),net=str(net),full=used==qty,gas=int(fill["gasUsed"],16),calls=calls,results=results)
        return prefix+[approval(0),approval(qty),exact],data
    settlements=[];rejections=[];stress_cases=[]
    for cash in ([BASE_WETH,"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"] if chain=="base" else
                 [ETH_WETH,"0xdac17f958d2ee523a2206206994597c13d831ec7"]):
        payer=HOLDERS[chain][cash]
        if treasury==payer or any(rpc(endpoint,"eth_getCode",[addr,tag])!="0x" for addr in [payer,EXEC]):raise ValueError("адрес контроля кошелька")
        rows=sorted((m for m in directory["markets"] if cash in m["tokens"] and int(m["liquidity"])),key=lambda m:-int(m["liquidity"]))
        for m in rows:
            other=next(t for t in m["tokens"] if t!=cash);measured=[];failed=False
            sizes=[10**12,10**14] if cash==weth else [100,10_000]
            for qty in sizes:
                for mode in ("router","router02","v5"):
                    prefix,buy=leg(m,payer,[],mode,cash,other,qty)
                    if prefix and buy["full"]:
                        _,sell=leg(m,payer,prefix,mode,other,cash,int(buy["net"]))
                    else:sell=None
                    if not (sell and sell.get("full")):
                        rejections.append(dict(pool=m["pool"],payer=payer,mode=mode,cash=cash,buy=buy,sell=sell))
                        failed=True;break
                    measured.append(dict(mode=mode,buy=buy,sell=sell))
                if failed:break
                if len({(c["buy"]["net"],c["sell"]["net"]) for c in measured[-3:]})!=1:raise ValueError("несовпадение режимов")
            if not failed:
                settlements.append(dict(pool=m["pool"],tokens=m["tokens"],fee=m["fee"],payer=payer,cash=cash,cases=measured))
                # Верхняя граница ограничена реальным балансом, без stateOverride плательщика.
                bal,=ret(run(payer,[tx(payer,cash,"balanceOf(address)",["address"],[payer])],"native")[0],["uint256"])
                large=min(bal,10**22 if cash==weth else 10**11)
                if large>sizes[-1]:
                    _,observation=leg(m,payer,[],"v5",cash,other,large)
                    stress_cases.append(dict(pool=m["pool"],payer=payer,input_asset=cash,observed=observation))
                break
        else:raise ValueError(f"нет двусторонней выплаты для {cash}; "+str([(v["pool"],v["mode"],v["buy"].get("reason"),(v["sell"] or {}).get("reason")) for v in rejections])[:450])
    return dict(chain=chain,selector=tag,state_root=head["stateRoot"],treasury=treasury,periphery=contracts,
                executor_hash="0x"+keccak(bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))).hex(),
                settlements=settlements,rejected=rejections,stress=stress_cases)

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("chain",choices=("base","ethereum"));ap.add_argument("output",type=Path);args=ap.parse_args()
    directory=json.loads((DATA/f"hello_{args.chain}_directory.json").read_bytes())
    for endpoint in (BASE_ENDPOINTS if args.chain=="base" else ETH_ENDPOINTS):
        try:
            proof=capture(directory,endpoint);args.output.write_text(json.dumps(proof,indent=2)+"\n",encoding="utf8");break
        except (KeyError,ValueError,OSError) as exc:print(endpoint,str(exc)[:650],flush=True)
    else:raise ValueError("нет подтверждённого двустороннего исполнения HELLO")
