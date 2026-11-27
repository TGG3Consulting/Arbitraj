"""Deterministic maker in an isolated canonical-state replay; this measures settlement, not public RFQ liquidity."""
import hashlib
from eth_abi import encode
from eth_keys import keys
from eth_utils import keccak
from record_aktionariat_wallet import FUND, SWAP as FUND_TUPLE, WETH, approve
from record_base_slipstream_executor import OWNER, rpc, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx

# Public, deterministic fixture identity; all assets come from wrapped simulated ETH.
MAKER = keys.PrivateKey(keccak(text="Arbitraj isolated AirSwap archive maker fixture"))
WALLET = MAKER.public_key.to_address()
UINT = "uint256"
ADDR = "address"
WIRE = [ADDR,UINT,UINT,ADDR,ADDR,UINT,ADDR,UINT,"uint8","bytes32","bytes32"]
HASH = "OrderERC20(uint256 nonce,uint256 expiry,address signerWallet,address signerToken,uint256 signerAmount,uint256 protocolFee,address senderWallet,address senderToken,uint256 senderAmount)"


def funded(url, network, at, end, swap, version, getter, compiled):
    base = network=="base"
    wrapped = "0x4200000000000000000000000000000000000006" if base else WETH
    usdc = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913" if base else "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
    router = "0x2626664c2603336e57b271c5c0b26f421741e481" if base else FUND
    fund_type = "(address,address,uint24,address,uint256,uint256,uint160)" if base else FUND_TUPLE
    fee = getter(swap,"protocolFee()",UINT)
    domain = getter(swap,"DOMAIN_SEPARATOR()","bytes32")
    type_hash = getter(swap,"ORDER_TYPEHASH()","bytes32")
    assert type_hash==keccak(text=HASH)
    seed=10**17
    market_buy=(wrapped,usdc,500,WALLET,*(() if base else (end,)),seed,2_000_000,0)
    env={OWNER:{"balance":hex(10**18)},WALLET:{"balance":hex(10**18)}}
    prep=[tx(wrapped,"deposit()",value=2*seed),approve(wrapped,router,seed),
          tx(router,"exactInputSingle("+fund_type+")",[fund_type],[market_buy])]
    module=None
    manifest=None
    if compiled:
        assert int(rpc(url,"eth_getTransactionCount",[OWNER,at]),16)==0
        module="0x"+keccak(bytes.fromhex("d694"+OWNER[2:]+"80"))[-20:].hex()
        assert rpc(url,"eth_getCode",[module,at])=="0x"
        creation="0x"+compiled["creation"]+encode([ADDR],[swap]).hex()
        deploy={"from":OWNER,"data":creation,"gas":hex(5_000_000)}
        created,owner,exchange,exchange_hash=simulate_block(url,at,env,[deploy,tx(module,"owner()"),tx(module,"exchange()"),tx(module,"exchangeHash()")])
        assert all(c["status"]=="0x1" for c in (created,owner,exchange,exchange_hash))
        assert int(owner["returnData"],16)==int(OWNER,16) and int(exchange["returnData"],16)==int(swap,16)
        runtime=bytes.fromhex(created["returnData"][2:])
        assert runtime
        manifest=dict(chainId=8453 if base else 1,kind="airswap_"+version,sourceBlock=at["blockHash"],
                      exchange=swap,exchangeHash=exchange_hash["returnData"],runtimeKeccak="0x"+keccak(runtime).hex(),
                      creationSha256=hashlib.sha256(bytes.fromhex(creation[2:])).hexdigest(),runtimeSha256=hashlib.sha256(runtime).hexdigest(),
                      data=creation,gas=int(created["gasUsed"],16),solc=compiled["solc"])
        prep=[deploy]+prep
    def payment(src,dst,amount,output,nonce,deadline,who=OWNER,protected=False,opened=False,minimum=1,until=None):
        data=encode(["bytes32",UINT,UINT,ADDR,ADDR,UINT,UINT,ADDR,ADDR,UINT],
                    [type_hash,nonce,deadline,WALLET,dst,output,fee,who,src,amount])
        signature=MAKER.sign_msg_hash(keccak(b"\x19\x01"+domain+keccak(data)))
        args=[OWNER,nonce,deadline,WALLET,dst,output,src,amount,signature.v+27,
              signature.r.to_bytes(32,"big"),signature.s.to_bytes(32,"big")]
        if protected:
            order_type="(uint256,uint256,address,address,uint256,address,uint256,bool,uint8,bytes32,bytes32)"
            order=(nonce,deadline,WALLET,src,amount,dst,output,opened,*args[-3:])
            return tx(module,"execute("+order_type+",uint256,uint256)",[order_type,UINT,UINT],[order,minimum,end if until is None else until])
        name="swapAnySender" if opened else "swap"
        return tx(swap,name+"("+",".join(WIRE)+")",WIRE,args)
    controls=[]
    for src,dst,amount,output in [(wrapped,usdc,10**15,1_000_000),(usdc,wrapped,1_000_000,10**15)]:
        # Seed both parties without overriding any ERC20 balances, proxy code or storage.
        initial=list(prep)
        if src==usdc:
            initial.extend([tx(wrapped,"deposit()",value=10**16)|{"from":WALLET},
                            tx(usdc,"transfer(address,uint256)",[ADDR,UINT],[OWNER,amount])|{"from":WALLET}])
        fee_amount=output*fee//10000
        initial.extend([approve(src,swap,amount),approve(dst,swap,output+fee_amount)|{"from":WALLET}])
        nonce=91
        checks=[balance(src,OWNER),balance(dst,OWNER),balance(src,WALLET),balance(dst,WALLET)]
        paid=payment(src,dst,amount,output,nonce,end)
        actions=[payment(src,dst,amount,output,nonce,1),
                 payment(src,dst,amount,output,nonce,end,WALLET),paid,paid]
        calls=simulate_block(url,at,env,initial+checks+actions+checks)
        i=len(initial)
        assert all(c["status"]=="0x1" for c in calls[:i+4]+calls[-4:]), "funding or balance failed"
        results=calls[i+4:i+8]
        assert [r["status"] for r in results]==["0x0","0x0","0x1","0x0"],[(r["status"],r.get("error")) for r in results]
        delta=[int(b["returnData"],16)-int(a["returnData"],16) for a,b in zip(calls[i:i+4],calls[-4:])]
        assert delta==[-amount,output,amount,-output-fee_amount],delta
        logs=results[2]["logs"]
        assert token_flow(logs,src,OWNER)==-amount and token_flow(logs,dst,OWNER)==output
        event_types="uint256,address" if version=="v5" else "uint256,address,address,uint256,uint256,address,address,uint256"
        topic="0x"+keccak(text="SwapERC20("+event_types+")").hex()
        events=[l for l in logs if l["address"].lower()==swap and l["topics"] and l["topics"][0]==topic]
        assert len(events)==1 and int(events[0]["topics"][1],16)==nonce and events[0]["topics"][2][-40:]==WALLET[2:]
        controls.append(dict(source="fixture maker",input=src,output=dst,spent=str(amount),received=str(output),
                             fee_paid_by_maker=str(fee_amount),gas=int(results[2]["gasUsed"],16),logs=logs,
                             refusals=[r["error"] for r in [results[0],results[1],results[3]]]))
        if module:
            maker_allowance=approve(dst,swap,output+fee_amount)|{"from":WALLET}
            # Same real input funds and original exchange; only the paying address differs.
            seeded=initial[:-2]+[approve(src,module,amount),maker_allowance]
            for opened in (False,True):
                sender="0x"+"00"*20 if opened else module
                def protected(**changes):
                    kw=dict(who=sender,protected=True,opened=opened,minimum=output)
                    return payment(src,dst,amount,output,nonce,end,**(kw|changes))
                good=protected()
                steps=[good|{"from":WALLET},protected(minimum=0),protected(minimum=output+1),protected(until=1),
                       protected(who=OWNER),good,good]
                extras=[balance(src,module),tx(src,"allowance(address,address)",[ADDR,ADDR],[module,swap])]
                view=checks+extras
                result=simulate_block(url,at,env,seeded+view+steps+view)
                idx=len(seeded)+len(view)
                assert all(c["status"]=="0x1" for c in result[:idx]+result[-len(view):])
                seq=result[idx:idx+len(steps)]
                assert [c["status"] for c in seq]==["0x0"]*5+["0x1","0x0"],[(c["status"],c.get("error")) for c in seq]
                changes=[int(b["returnData"],16)-int(a["returnData"],16) for a,b in zip(result[len(seeded):idx],result[-len(view):])]
                assert changes==delta+[0,0] and int(result[-1]["returnData"],16)==0
                cash=seq[5]; logs=cash["logs"]
                paid_topic="0x"+keccak(text="Paid(address,address,uint256,address,address,uint256,uint256)").hex()
                assert sum(l["address"].lower()==module and l["topics"][0]==paid_topic for l in logs if l["topics"])==1
                assert sum(l["address"].lower()==swap and l["topics"][0]==topic for l in logs if l["topics"])==1
                assert token_flow(logs,src,OWNER)==-amount and token_flow(logs,dst,OWNER)==output
                controls.append(dict(source="fixture maker",mode="wallet_any" if opened else "wallet_bound",
                                     module=module,input=src,output=dst,spent=str(amount),received=str(output),gas=int(cash["gasUsed"],16),
                                     nonce=str(nonce),payer=OWNER,maker=WALLET,logs=logs,refusals=[c["error"] for c in seq if c["status"]=="0x0"]))
    return controls,manifest
