"""Сверяет реестр EtherVista и реальные дельты ETH и токенов в изолированном архивном блоке."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, OWNER, call, calldata, rpc, simulate_block
from record_ethereum_v2_factories import registry

NETWORKS = {
    "base": ("0x9c9dfc8b5d8f1cb1b7f854108db16be1c21ea400","0x4aab7db193bf3d0083e9db51735ab3da0328c3d7",
             "0x4200000000000000000000000000000000000006","0x14ed3c77ea87fd259f590e3050a245adeb8c60131fe112f3a1610a316a5c076d",
             ["https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"]),
    "ethereum": ("0x9a27cb5ae0b2cee0bb71f9a85c0d60f3920757b4","0x9bd63c5d44ff28390df1eaafd4eb4bd73e94a72a",
                 "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2","0xe260b72768e8ec6814aa811c576f346d208ba00840f835949d65c6424ac80a8d",
                 ["https://eth.drpc.org","https://ethereum-rpc.publicnode.com","https://ethereum.publicnode.com"]),
}
PREVIOUS_ROUTERS = {"base":"0x018004f8b21b3511d98b1bbffaee034f58ae288e",
                    "ethereum":"0xcedd366065a146a039b92db35756ecd7688fcc77"}
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"


def tx(to,sig,types=(),args=(),value=0):
    return call(to,calldata(sig,types,args),value)


def integer(reply):
    if reply["status"] != "0x1":
        raise ValueError(f"read failed: {reply.get('error')}")
    return decode(["uint256"],bytes.fromhex(reply["returnData"][2:]))[0]


def provenance(network,address,c):
    if network=="base":
        url=f"https://sourcify.dev/server/v2/contract/8453/{address}"
        res=requests.get(url,params={"fields":"compilation,runtimeBytecode.onchainBytecode"},timeout=25)
        res.raise_for_status()
        data=res.json()
        verified=data["runtimeBytecode"]["onchainBytecode"]
        name=data["compilation"]["fullyQualifiedName"]
    else:
        url="https://eth.blockscout.com/api/v2/smart-contracts/"+address
        res=requests.get(url,timeout=30)
        res.raise_for_status()
        data=res.json()
        if not data["is_verified"] or data["is_changed_bytecode"]:
            raise ValueError("explorer did not verify the deployed contract")
        verified=data["deployed_bytecode"]
        name=data["name"]
    code=rpc(c.endpoint,"eth_getCode",[address,c.selector])
    if code=="0x" or code.lower()!=verified.lower():
        raise ValueError("archive runtime differs from verified source: "+address)
    return dict(source=url,name=name,runtime_hash="0x"+keccak(bytes.fromhex(code[2:])).hex())


def settlement(c,head,row,token,weth,router,previous):
    pool=row["pool"]
    at=int(head["timestamp"],16)
    ttl=at+120
    observer="0x"+keccak(text="unfunded EtherVista payout observer")[-20:].hex()
    def eth(who):
        return tx(MULTICALL,"getEthBalance(address)",["address"],[who])
    def erc(asset,who):
        return tx(asset,"balanceOf(address)",["address"],[who])
    def min_buy(qty,minimum):
        return tx(router,"swapExactETHForTokensSupportingFeeOnTransferTokens(uint256,address[],address,uint256)",
                  ["uint256","address[]","address","uint256"],[minimum,[weth,token],OWNER,ttl],qty)
    def min_sell(qty,minimum,to,fee):
        return tx(router,"swapExactTokensForETHSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                  ["uint256","uint256","address[]","address","uint256"],[qty,minimum,[token,weth],to,ttl],fee)
    reads=[eth(OWNER),erc(token,OWNER),eth(observer),erc(weth,router),
           tx(token,"allowance(address,address)",["address","address"],[OWNER,router])]
    def execute(prefix,order):
        emissions=simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(100*10**18)}},prefix+reads+[order]+reads)
        if any(x["status"]!="0x1" for x in emissions[:len(prefix)]):
            raise ValueError("accepted prefix differs from the original overlay")
        before=list(map(integer,emissions[len(prefix):len(prefix)+len(reads)]))
        fill=emissions[len(prefix)+len(reads)]
        after=list(map(integer,emissions[-len(reads):]))
        changes=[b-a for a,b in zip(before,after,strict=True)]
        if fill["status"]=="0x0" and any(changes[1:]):
            raise ValueError("revert changed tokens, receiver or allowance")
        return dict(status=fill["status"],error=fill.get("error"),gas=int(fill["gasUsed"],16),
                    wallet_native=str(changes[0]), wallet_tokens=str(changes[1]), receiver_native=str(changes[2]),
                    router_weth_before=str(before[3]),router_weth_after=str(after[3]))
    components={}
    for side in ("buy","sell"):
        units,=c.one(pool,side+"TotalFee()",returns=["uint256"])
        native,=c.one(router,"usdcToEth(uint256)",["uint256"],[units],["uint256"])
        components[side]=dict(units=units,native_wei=native)
    fee_in=components["buy"]["native_wei"]
    fee_out=components["sell"]["native_wei"]
    reserve=int(row["reserves"][row["tokens"].index(weth)])
    sizes=sorted({min(10**15,reserve//1000),min(10**16,reserve//100)})
    result=dict(fees=components,swaps=[])
    for qty in sizes:
        if qty<=0:continue
        total=fee_in+qty
        (quoted,)=c.one(router,"getAmountsOut(uint256,address[])",["uint256","address[]"],[qty,[weth,token]],["uint256[]"])
        buy_order=min_buy(total,1)
        buy=execute([],buy_order)
        rec=dict(net_eth_for_pool=str(qty),total_buy_eth=str(total),native_buy_quote=str(quoted[-1]),buy=buy)
        result["swaps"].append(rec)
        if buy["status"]!="0x1":continue
        volume=int(buy["wallet_tokens"])
        if volume<=0 or int(buy["wallet_native"])>-total:
            raise ValueError("token or ETH buy delta incorrect")
        accepted=min_buy(total,volume)
        rec["buy_at_measured"]=execute([],accepted)
        rec["previous_router_same_buy"]=execute([],dict(accepted,to=previous))
        rec["buy_min_plus_one"]=execute([],min_buy(total,volume+1))
        rec["buy_only_fee"]=execute([],min_buy(fee_in,1))
        if (rec["buy_at_measured"]["status"],rec["buy_min_plus_one"]["status"],rec["buy_only_fee"]["status"])!=("0x1","0x0","0x0"):
            raise ValueError("native buy floor or fixed fee boundary differs")
        approval=tx(token,"approve(address,uint256)",["address","uint256"],[router,volume])
        prefix=[accepted,approval]
        recipient=execute(prefix,min_sell(volume,1,observer,fee_out))
        rec["sell_receiver"]=recipient
        if recipient["status"]!="0x1":continue
        gross=int(recipient["receiver_native"])
        if gross<=0 or int(recipient["wallet_tokens"])!=-volume:
            rec["sell_wallet_debit_differs"]=True
            continue
        own=execute(prefix,min_sell(volume,gross,OWNER,fee_out))
        strict=execute(prefix,min_sell(volume,gross+1,OWNER,fee_out))
        rec.update(sell_same_wallet=own,sell_min_plus_one=strict,measured_gross_wei=str(gross))
        if fee_out:
            under=execute(prefix,min_sell(volume,1,OWNER,fee_out-1))
            rec["sell_fee_minus_one"]=under
            if under["status"]!="0x0":raise ValueError("underfunded sell accepted")
        if own["status"]!="0x1" or strict["status"]!="0x0" or int(own["wallet_tokens"])!=-volume:
            raise ValueError("native sell floor differs from observed wallet settlement")
        rec["sell_network_charge_in_simulation"]=str(gross-fee_out-int(own["wallet_native"]))
    return result


def directory(network,c,snapshot,head):
    factory,router,weth,init,_=NETWORKS[network]
    for target,sig,wanted in ((factory,"router()",router),(router,"factory()",factory),(router,"WETH()",weth)):
        found,=c.one(target,sig,returns=["address"])
        if found!=wanted:
            raise ValueError(f"{target} {sig}: expected {wanted}, received {found}")
    previous=PREVIOUS_ROUTERS[network]
    historical_binding,=c.one(previous,"factory()",returns=["address"])
    verified={a:provenance(network,a,c) for a in (factory,router)}
    (init_code,)=c.one(factory,"INIT_CODE_HASH()",returns=["bytes32"])
    if "0x"+init_code.hex()!=init:raise ValueError("registry and verified router init code disagree")
    pools=registry(c,factory,init)
    eligible=[p for p in pools if weth in p["tokens"] and all(int(r)>0 for r in p["reserves"])]
    eligible.sort(key=lambda p:-int(p["reserves"][p["tokens"].index(weth)]))
    attempts=[]
    for row in eligible:
        token,=[t for t in row["tokens"] if t!=weth]
        res=settlement(c,head,row,token,weth,router,previous)
        attempts.append(dict(pool=row["pool"],token=token,**res))
        if len(res["swaps"])==2 and all("sell_same_wallet" in case for case in res["swaps"]):break
    return dict(network=network,block=snapshot["block"],selector=c.selector,state_root=head["stateRoot"],
                factory=factory,router=router,previous_router=previous,previous_router_factory=historical_binding,
                weth=weth,init=init,verified=verified,registry=pools,
                funded_native_pairs=len(eligible),observations=attempts)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("network",choices=NETWORKS)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    snap=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())[args.network]
    for ep in NETWORKS[args.network][-1]:
        try:
            c=Capture(dict(endpoint=ep,selector={"blockHash":snap["hash"],"requireCanonical":True}))
            head=rpc(ep,"eth_getBlockByHash",[snap["hash"],False])
            if head["hash"]!=snap["hash"] or int(head["number"],16)!=snap["block"] or int(rpc(ep,"eth_chainId",[]),16)!=snap["chain"]:
                raise ValueError("wrong archive network or height")
            out=directory(args.network,c,snap,head)
            args.output.write_text(json.dumps(out,indent=2)+"\n",encoding="utf8")
            break
        except (requests.RequestException, ValueError, RuntimeError, KeyError) as error:
            print(ep,str(error)[:500],flush=True)
    else:
        raise ValueError("unable to reconstruct the canonical EtherVista market")
