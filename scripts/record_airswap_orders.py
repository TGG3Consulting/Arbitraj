"""Deterministic maker in an isolated canonical-state replay; this measures settlement, not public RFQ liquidity."""
from eth_abi import encode
from eth_keys import keys
from eth_utils import keccak
from record_aktionariat_wallet import FUND, SWAP as FUND_TUPLE, WETH, approve
from record_base_slipstream_executor import OWNER, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx

# Public, deterministic fixture identity; all assets come from wrapped simulated ETH.
MAKER = keys.PrivateKey(keccak(text="Arbitraj isolated AirSwap archive maker fixture"))
WALLET = MAKER.public_key.to_address()
UINT = "uint256"
ADDR = "address"
WIRE = [ADDR,UINT,UINT,ADDR,ADDR,UINT,ADDR,UINT,"uint8","bytes32","bytes32"]
HASH = "OrderERC20(uint256 nonce,uint256 expiry,address signerWallet,address signerToken,uint256 signerAmount,uint256 protocolFee,address senderWallet,address senderToken,uint256 senderAmount)"


def funded(url, network, at, end, swap, version, getter):
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
    def payment(src,dst,amount,output,nonce,deadline,who=OWNER):
        data=encode(["bytes32",UINT,UINT,ADDR,ADDR,UINT,UINT,ADDR,ADDR,UINT],
                    [type_hash,nonce,deadline,WALLET,dst,output,fee,who,src,amount])
        signature=MAKER.sign_msg_hash(keccak(b"\x19\x01"+domain+keccak(data)))
        args=[OWNER,nonce,deadline,WALLET,dst,output,src,amount,signature.v+27,
              signature.r.to_bytes(32,"big"),signature.s.to_bytes(32,"big")]
        return tx(swap,"swap("+",".join(WIRE)+")",WIRE,args)
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
    return controls
