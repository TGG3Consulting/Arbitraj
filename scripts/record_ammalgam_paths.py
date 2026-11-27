"""Full Ethereum wallet payments through two different registered Ammalgam pools."""
from eth_utils import keccak
from record_aktionariat_wallet import FUND, SWAP, WETH, approve
from record_base_slipstream_executor import EXEC, OWNER, STEP, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx

USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
FILLED = "0x" + keccak(text="Filled(address,address,address,address,address,uint256,uint256,uint256)").hex()
SWAPPED = "0x" + keccak(text="Swap(address,uint256,uint256,uint256,uint256,address)").hex()


def paths(url, tag, env, prefix, module, end, usdt, pools):
    records = []
    for src, dst, amount in [(WETH, usdt, 10**15), (usdt, WETH, 10**6)]:
        tokens = [src, USDC, dst]
        pairs = [next(p["pool"] for p in pools if set(p["tokens"]) == {a,b}) for a,b in zip(tokens,tokens[1:])]
        assert len(set(pairs)) == 2
        seed = 2 * 10**17
        prep = prefix + [tx(WETH, "deposit()", value=seed)]
        if src != WETH:
            prep += [approve(WETH, FUND, seed), tx(FUND, "exactInputSingle("+SWAP+")", [SWAP],
                     [(WETH, src, 3000, OWNER, end, seed, amount, 0)])]
        prep += [approve(src, EXEC, amount)]
        # Wallet, executor and module must all agree on the intermediate balances.
        checks = [balance(t,OWNER) for t in tokens] + [balance(USDC,a) for a in [EXEC,module]]
        steps = [(29,module,a,b,10000,b"") for a,b in zip(tokens,tokens[1:])]

        def swap(minimum, deadline):
            return tx(EXEC, "execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                      ["address","uint256","address","uint256","uint256",STEP+"[]"],
                      [src,amount,dst,minimum,deadline,steps])

        def replay(ops):
            result = simulate_block(url,tag,env,prep+checks+ops+checks)
            i = len(prep)+len(checks)
            assert all(row["status"]=="0x1" for row in result[:i]+result[-len(checks):])
            before = [int(row["returnData"],16) for row in result[len(prep):i]]
            after = [int(row["returnData"],16) for row in result[-len(checks):]]
            return result[i:-len(checks)], [b-a for a,b in zip(before,after)]

        (call,), delta = replay([swap(1,end)])
        assert call["status"]=="0x1", call.get("error")
        received=delta[2]
        assert received>0 and delta==[-amount,0,received,0,0]
        logs=call["logs"]
        assert [l["address"].lower() for l in logs if l["topics"][0]==SWAPPED and l["address"].lower() in pairs]==pairs
        assert sum(l["topics"][0]==FILLED and l["address"].lower()==module for l in logs)==2
        assert token_flow(logs,src,OWNER)==-amount and token_flow(logs,dst,OWNER)==received
        controls, after = replay([swap(received+1,end),swap(received,1),swap(received,end)])
        assert [c["status"] for c in controls]==["0x0","0x0","0x1"] and after==delta
        records.append(dict(tokens=tokens,pairs=pairs,spent=str(amount),received=str(received),
                            gas=int(call["gasUsed"],16),wallet_and_module_intermediate_changes=[0,0,0],
                            minimum_refusal=controls[0]["error"],expired=controls[1]["error"],logs=logs))
    return records
