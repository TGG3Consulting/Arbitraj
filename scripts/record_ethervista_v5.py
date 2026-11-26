"""Проверяет полный ERC20-маршрут V5 в обе стороны через активный EtherVista router и средства его владельца."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block
from record_ethervista_wallets import MULTICALL, NETWORKS, integer, tx

ADAPTER = "0x" + keccak(text="isolated EtherVista adapter")[-20:].hex()
BANK = "0x" + keccak(text="isolated EtherVista reserve owner")[-20:].hex()
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
BASE_USDT = "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2"
BASE_USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
PAYEE = "0x" + keccak(text="isolated EtherVista native recipient")[-20:].hex()
EVENT = "0x" + keccak(text="NativeSettled(address,address,address,address,uint256,uint256,uint256)").hex()


def bound(artifact, directory, endpoint):
    args = encode(["address","address"],[directory["router"],BANK])
    response=requests.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":[
        {"data":"0x"+artifact["creation"]+args.hex()},directory["selector"]]},timeout=30)
    response.raise_for_status()
    runtime=response.json()["result"]
    compiled = bytes.fromhex(artifact["runtime"])
    observed = bytearray.fromhex(runtime[2:])
    if len(compiled) != len(observed):
        raise ValueError("constructor bytecode length differs")
    addresses = set()
    for refs in artifact["immutableReferences"].values():
        words = set()
        for ref in refs:
            i = ref["start"]
            if ref["length"] != 32 or len(observed[i:i+32]) != 32:
                raise ValueError("unrecognized immutable reference")
            words.add(bytes(observed[i:i+32]))
            observed[i:i+32] = bytes(32)
        if len(words) != 1:
            raise ValueError("immutable has conflicting values")
        addresses.add(words.pop())
    expected = {encode(["address"],[a]) for a in (BANK,directory["router"],directory["factory"],directory["weth"])}
    if observed != compiled or addresses != expected:
        raise ValueError("runtime or bound contracts differ from compiled bytecode")
    return runtime


def asset_path(directory, endpoint, cash):
    net = directory["network"]
    weth = directory["weth"]
    profiles = json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    uni = next(p["settings"] for p in profiles if p["network"]==net and p["settings"]["dex.kind"]=="uniswap_v2")
    c = Capture(dict(endpoint=endpoint,selector=directory["selector"]))
    if cash==BASE_USDT and c.one(cash,"l1Token()",returns=["address"])[0]!=USDT:
        raise ValueError("bridged USDT has another L1 asset")
    label = c.one(cash,"symbol()",returns=["string"])[0]
    if label != ("USDC" if cash==BASE_USDC else "USDT") or c.one(cash,"decimals()",returns=["uint8"])[0]!=6:
        raise ValueError("wrong wallet settlement token")
    tokens = [cash,BASE_USDC,weth] if cash==BASE_USDT else [cash,weth]
    step_data=encode(["uint32"],[3000])
    steps=[]
    for a,b in zip(tokens,tokens[1:]):
        pool, = c.one(uni["dex.factory"],"getPair(address,address)",["address","address"],[a,b],["address"])
        if int(pool,16)==0 or c.one(pool,"factory()",returns=["address"])[0]!=uni["dex.factory"].lower():
            raise ValueError("missing verified funding market")
        steps.append((0,pool,a,b,10000,step_data))
    return tokens,steps,uni["dex.router"]


def scenario(directory, artifact, endpoint, cash):
    tag = directory["selector"]
    head = rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if int(rpc(endpoint,"eth_chainId",[]),16)!={"base":8453,"ethereum":1}[directory["network"]] or head["stateRoot"]!=directory["state_root"]:
        raise ValueError("wrong state or network")
    runtime = bound(artifact,directory,endpoint)
    v5 = RUNTIME.read_text().strip().removeprefix("0x")
    word = lambda n:"0x"+format(n,"064x")
    env = {OWNER:{"balance":hex(10**20)}, BANK:{"balance":hex(10**19)}, ADAPTER:{"code":runtime},
           EXEC:{"code":"0x"+v5,"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(directory["weth"],16))}}}
    market = directory["observations"][0]
    approval_proof={}
    if directory["network"]=="ethereum":
        original=market["token"]
        def spender_answer(target):
            approve=tx(original,"approve(address,uint256)",["address","uint256"],[target,100])
            resp=simulate_block(endpoint,tag,env,[approve])[0]
            return {"status":resp["status"],"data":resp["returnData"],"error":resp.get("error")}
        approval_proof={"token":original,"executor":spender_answer(EXEC),"router":spender_answer(directory["router"])}
        if approval_proof["executor"]["status"]!="0x0" or approval_proof["router"]["status"]!="0x1":
            raise ValueError("historical VISTA allowance boundary differs")
        # Следующий по WETH-резерву рынок EtherVista. Владельцу нужен отдельный допуск каждой монеты.
        pair=next(p for p in directory["registry"] if p["pool"]=="0xd0a9bb124c4611ec375d93b5e9f20156c73b2004")
        token=next(t for t in pair["tokens"] if t!=directory["weth"])
        c=Capture(dict(endpoint=endpoint,selector=tag))
        fees={}
        for side in ("buy","sell"):
            units,=c.one(pair["pool"],side+"TotalFee()",returns=["uint256"])
            wei,=c.one(directory["router"],"usdcToEth(uint256)",["uint256"],[units],["uint256"])
            fees[side]=dict(units=units,native_wei=wei)
        market=dict(pool=pair["pool"],token=token,fees=fees)
    asset,weth,router = market["token"],directory["weth"],directory["router"]
    tokens,steps,fund_router=asset_path(directory,endpoint,cash)
    steps.append((29,ADAPTER,weth,asset,10000,b""))
    # У срока остаётся запас на дочерний блок симуляции. Пределы бота не используются.
    deadline=int(head["timestamp"],16)+600
    fee=int(market["fees"]["sell"]["native_wei"])
    balance=lambda token,owner:tx(token,"balanceOf(address)",["address"],[owner])
    eth=lambda owner:tx(MULTICALL,"getEthBalance(address)",["address"],[owner])
    allowance=lambda token,a,b:tx(token,"allowance(address,address)",["address","address"],[a,b])
    reserve=lambda:tx(ADAPTER,"feeReserve()")
    def owner_call(q):
        return dict(q,**{"from":BANK})
    def deposit(q):
        return owner_call(tx(ADAPTER,"fundReserve()",value=q))
    def approve(t,n):
        return tx(t,"approve(address,uint256)",["address","uint256"],[EXEC,n])
    def route(forward,n,floor):
        walk=steps if forward else [(k,p,b,a,share,data) for k,p,a,b,share,data in reversed(steps)]
        call=tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                ["address","uint256","address","uint256","uint256",STEP+"[]"],
                [walk[0][2],n,walk[-1][3],floor,deadline,walk])
        call["gas"]=hex(2_500_000)
        return call
    cash_contracts=[cash,weth,asset]+([BASE_USDC] if cash==BASE_USDT else [])
    reads={"cash":balance(cash,OWNER),"coin":balance(asset,OWNER),"reserve":reserve(),
           "module_native":eth(ADAPTER),"router_weth":balance(weth,router),"pair_weth":balance(weth,market["pool"]),
           "module_approval":allowance(asset,ADAPTER,router),"payee_native":eth(PAYEE),
           "owner_native":eth(OWNER),"bank_native":eth(BANK)}
    for token in cash_contracts:
        reads["v5_"+token]=balance(token,EXEC)
        reads["adapter_"+token]=balance(token,ADAPTER)
        reads["step_approval_"+token]=allowance(token,EXEC,ADAPTER)
    def trial(before,call):
        query=list(reads.values())
        result=simulate_block(endpoint,tag,env,[*before,*query,call,*query])
        for i, row in enumerate(result[:len(before)]):
            if row["status"]!="0x1":
                raise ValueError(f"setup call {i} reverted: {row.get('error')}")
        index=len(before)+len(query)
        rc=result[index]
        keys=tuple(reads)
        a=dict(zip(keys,map(integer,result[len(before):index])))
        b=dict(zip(keys,map(integer,result[index+1:])))
        if rc["status"]=="0x0" and any(a[k]!=b[k] for k in a if k not in ("owner_native","bank_native")):
            raise ValueError("revert moved funds")
        if any(b[k] for k in keys if k.startswith(("step_approval_","v5_","adapter_")) or k=="module_approval"):
            raise ValueError("residue or allowance after transaction")
        events=[]
        for log in rc.get("logs",[]):
            if log["address"].lower()==ADAPTER and log["topics"][:1]==[EVENT]:
                money=decode(["address","uint256","uint256","uint256"],bytes.fromhex(log["data"][2:]))
                events.append(dict(payer="0x"+log["topics"][1][-40:],recipient="0x"+log["topics"][2][-40:],
                                   token_in="0x"+log["topics"][3][-40:],token_out=money[0],
                                   spent=str(money[1]),received=str(money[2]),fee=str(money[3])))
        return dict(status=rc["status"],gas=rc["gasUsed"],error=rc.get("error"),settled=events,
                    before={k:str(v) for k,v in a.items()},after={k:str(v) for k,v in b.items()})
    def delta(row,key):return int(row["after"][key])-int(row["before"][key])
    def successful(before,forward,amount):
        spend,receipt=("cash","coin") if forward else ("coin","cash")
        probe=trial(before,route(forward,amount,1))
        if probe["status"]!="0x1":raise ValueError(f"funded route rejected {forward} {amount}: {probe['error']}")
        paid=delta(probe,receipt)
        exact=trial(before,route(forward,amount,paid))
        upper=trial(before,route(forward,amount,paid+1))
        if (exact["status"],upper["status"],delta(exact,spend),delta(exact,receipt))!=("0x1","0x0",-amount,paid):
            raise ValueError("source, recipient or floor differs")
        if any(delta(exact,k) for k in ("reserve","module_native","router_weth")):
            raise ValueError("fee reserve or original router balance changed")
        return dict(amount=str(amount),net=str(paid),exact=exact,plus_one=upper),route(forward,amount,paid)

    def funding(size):
        return tx(fund_router,"swapETHForExactTokens(uint256,address[],address,uint256)",
                  ["uint256","address[]","address","uint256"],[size,list(reversed(tokens)),OWNER,deadline],10**18)
    cases=[]
    for size in (50_000_000,75_000_000):
        before=[funding(size),approve(cash,size)]
        buy,first=successful(before,True,size)
        if int(buy["exact"]["before"]["reserve"])!=0:raise ValueError("buy should not need a reserve")
        amount=int(buy["net"])
        prefix=[*before,first,approve(asset,amount)]
        deficient=trial([*prefix,deposit(fee-1)],route(False,amount,1))
        if deficient["status"]!="0x0":raise ValueError("sell spent an unfunded fee")
        sell,_=successful([*prefix,deposit(fee)],False,amount)
        dust=777_777_777_777
        add_dust=[tx(weth,"deposit()",value=dust),tx(weth,"transfer(address,uint256)",["address","uint256"],[router,dust])]
        restored,_=successful([*prefix,*add_dust,deposit(fee)],False,amount)
        if (int(restored["exact"]["before"]["router_weth"])-int(sell["exact"]["before"]["router_weth"])!=dust
                or sell["net"]!=restored["net"]):
            raise ValueError("router dust changed quoted net")
        cases.append(dict(cash_amount=str(size),buy=buy,sell=sell,insufficient_reserve=deficient,router_dust_sell=restored))
    native_trials=[]
    native_pair=next((m for m in directory["registry"] if {weth,cash}==set(m["tokens"])),None)
    if native_pair is not None:
        c=Capture(dict(endpoint=endpoint,selector=tag))
        pair=native_pair["pool"]
        def charge(side):
            n,=c.one(pair,side+"TotalFee()",returns=["uint256"])
            return c.one(router,"usdcToEth(uint256)",["uint256"],[n],["uint256"])[0]
        buy_fee,sell_fee=charge("buy"),charge("sell")
        reads.update(coin=balance(cash,OWNER),pair_weth=balance(weth,pair),module_approval=allowance(cash,ADAPTER,router))
        def native(forward,amount,minimum,advance):
            a,b=(weth,cash) if forward else (cash,weth)
            who=OWNER if forward else PAYEE
            return tx(ADAPTER,"swapNative(address,address,uint256,uint256,address,uint256)",
                      ["address","address","uint256","uint256","address","uint256"],[a,b,amount,minimum,who,deadline],advance)
        def official(forward,amount,minimum):
            if forward:
                return tx(router,"swapExactETHForTokensSupportingFeeOnTransferTokens(uint256,address[],address,uint256)",
                          ["uint256","address[]","address","uint256"],[minimum,[weth,cash],OWNER,deadline],amount)
            return tx(router,"swapExactTokensForETHSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                      ["uint256","uint256","address[]","address","uint256"],[amount,minimum,[cash,weth],PAYEE,deadline],sell_fee)
        def event_check(row,forward,amount,received):
            a,b=(weth,cash) if forward else (cash,weth)
            event=dict(payer=OWNER,recipient=OWNER if forward else PAYEE,token_in=a,token_out=b,
                       spent=str(amount),received=str(received),fee=str(buy_fee if forward else sell_fee))
            if row["settled"]!=[event] or any(delta(row,k) for k in ("module_native","reserve","router_weth")):
                raise ValueError(f"native receipt or ownership differs: {row['settled']} expected {event}")
        for pool_input in (10**15,10**16):
            budget=pool_input+buy_fee
            result=trial([],native(True,budget,1,budget))
            volume=delta(result,"coin")
            if result["status"]!="0x1" or volume<=0:raise ValueError("EOA buy rejected")
            buy_tx=native(True,budget,volume,budget)
            buy_native=trial([],buy_tx)
            buy_official=trial([],official(True,budget,volume))
            event_check(buy_native,True,budget,volume)
            if delta(buy_official,"coin")!=volume:raise ValueError("native buy differs from official payout")
            approval=lambda target:tx(cash,"approve(address,uint256)",["address","uint256"],[target,volume])
            prefix=[buy_tx,approval(ADAPTER)]
            official_sell=trial([buy_tx,approval(router)],official(False,volume,1))
            if official_sell["status"]!="0x1":raise ValueError("reference sell failed")
            dust0=int(official_sell["before"]["router_weth"])
            net=delta(official_sell,"payee_native")-dust0-sell_fee
            row=dict(cash=cash,pool=pair,budget=str(budget),buy_module=buy_native,buy_router=buy_official,
                     buy_min_plus_one=trial([],native(True,budget,volume+1,budget)),sell_router=official_sell,
                     expected_sell_net=str(net),buy_fee=str(buy_fee),sell_fee=str(sell_fee),sell_recipient=PAYEE)
            if row["buy_min_plus_one"]["status"]!="0x0":raise ValueError("native buy floor accepted")
            if net<=0:
                row["sell_module"]=trial(prefix,native(False,volume,1,sell_fee))
                if row["sell_module"]["status"]!="0x0":raise ValueError("negative net accepted")
            else:
                row["sell_module"]=trial(prefix,native(False,volume,net,sell_fee))
                event_check(row["sell_module"],False,volume,net)
                row["native_guards"]={
                    "one_more_sell":trial(prefix,native(False,volume,net+1,sell_fee)),
                    "one_less_advance":trial(prefix,native(False,volume,net,sell_fee-1)),
                    "one_more_advance":trial(prefix,native(False,volume,net,sell_fee+1)),
                }
                if any(v["status"]!="0x0" for v in row["native_guards"].values()):
                    raise ValueError("native sell floor or exact advance accepted")
                pad=123_456_789
                top=[tx(weth,"deposit()",value=pad),tx(weth,"transfer(address,uint256)",["address","uint256"],[router,pad])]
                row["sell_dust"]=trial([*prefix,*top],native(False,volume,net,sell_fee))
                event_check(row["sell_dust"],False,volume,net)
                if delta(row["sell_dust"],"payee_native")!=net+sell_fee:raise ValueError("router dust paid to EOA")
            native_trials.append(row)
    unauthorized=trial([deposit(fee)],tx(ADAPTER,"withdrawReserve(uint256)",["uint256"],[fee]))
    release=trial([deposit(fee)],owner_call(tx(ADAPTER,"withdrawReserve(uint256)",["uint256"],[fee])))
    if unauthorized["status"]!="0x0" or (delta(release,"reserve"),delta(release,"module_native"))!=(-fee,-fee):
        raise ValueError("reserve ownership differs")
    return dict(network=directory["network"],chain=int(rpc(endpoint,"eth_chainId",[]),16),selector=tag,
                recipient=OWNER,reserve_owner=BANK,router=router,cash=cash,path=[*tokens,asset],
                native=native_trials,original_approval_proof=approval_proof,
                pools=[s[1] for s in steps[:-1]]+[market["pool"]],reserve_wei=str(fee),owner_withdrawal=release,
                unauthorized_withdrawal=unauthorized,module_runtime_hash="0x"+keccak(bytes.fromhex(runtime[2:])).hex(),
                v5_runtime_hash="0x"+keccak(bytes.fromhex(v5)).hex(),cases=cases)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    for key in ("directory","artifact","output"):parser.add_argument(key,type=Path)
    args=parser.parse_args()
    directory=json.loads(args.directory.read_bytes());artifact=json.loads(args.artifact.read_bytes())
    network=directory["network"]; funds=[BASE_USDC,BASE_USDT] if network=="base" else [USDT]
    for endpoint in NETWORKS[network][-1]:
        try:
            cases=[scenario(directory,artifact,endpoint,cash) for cash in funds]
            args.output.write_text(json.dumps(cases,indent=2)+"\n",encoding="utf8")
            break
        except (requests.RequestException, ValueError, RuntimeError, KeyError) as exc:
            print(endpoint,str(exc)[:420],flush=True)
    else:raise ValueError("no canonical EtherVista settlement recording")
