"""Compare one official fee assessment with the same two physical pairs as separate V5 steps."""
from eth_abi import encode
from record_base_slipstream_executor import EXEC, OWNER, STEP, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx
from record_motoswap import ARGS, FEE, FUNDING, STABLES, SWAP, WETH, approve

MOTO = "0xbd965230588eaa536de6aa45e8ebbc01638535e0"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"


def compare(reader, directory, module, states, deadline):
    pools = {frozenset(p["tokens"]):p["pool"] for p in directory["pairs"]}
    rows = []
    for source, target in [(USDT, WETH), (WETH, USDT), (USDT, STABLES[0])]:
        if source == target:
            continue
        path = [source, MOTO, target]
        markets = [pools[frozenset((a,b))] for a,b in zip(path,path[1:])]
        if markets[0] == markets[1]:
            raise ValueError("same source pool")
        for amount in ([10**15,10**16] if source == WETH else [10**6,10**7]):
            funding = ([tx(WETH,"deposit()",value=amount)] if source == WETH else [
                tx(FUNDING, "swapETHForExactTokens(uint256,address[],address,uint256)",
                   ["uint256","address[]","address","uint256"],[amount,[WETH,source],OWNER,deadline],value=10**18)])
            marks = [balance(t,OWNER) for t in path]
            steps = [(17,module,a,b,10000,encode(["address"],[pool]))
                     for a,b,pool in zip(path,path[1:],markets)]
            for mode in ("original","guarded","v5_steps"):
                spender = {"original":FEE,"guarded":module,"v5_steps":EXEC}[mode]
                def order(minimum, until):
                    if mode == "original":
                        return tx(FEE,SWAP,ARGS,[amount,minimum,path,OWNER,until])
                    if mode == "guarded":
                        return tx(module,"executePath(address[],address[],uint256,uint256,uint256)",
                                  ["address[]","address[]","uint256","uint256","uint256"],[markets,path,amount,minimum,until])
                    return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                              ["address","uint256","address","uint256","uint256",STEP+"[]"],
                              [source,amount,target,minimum,until,steps])
                prefix = funding+[approve(source,spender,amount)]
                def run(calls):
                    results = simulate_block(None,reader.tag,states,prefix+marks+calls+marks,
                                             read_rpc=lambda _,method,args:reader.one(method,args))
                    if any(r["status"]!="0x1" for r in results[:len(prefix)+3]+results[-3:]):
                        raise ValueError(("funding or balance",mode,path,amount))
                    before = results[len(prefix):len(prefix)+3]
                    delta = [int(z["returnData"],16)-int(a["returnData"],16)
                             for a,z in zip(before,results[-3:],strict=True)]
                    return results[len(prefix)+3:-3],delta,int(results[len(prefix)-1]["gasUsed"],16)
                sample, observed, _ = run([order(1,deadline)])
                if sample[0]["status"]!="0x1" or observed[:2]!=[-amount,0] or observed[-1]<=0:
                    raise ValueError(("size unavailable",mode,path,amount,sample[0].get("error"),observed))
                net=observed[-1]
                trade=order(net,deadline)
                (high,expired,paid),settled,grant=run([order(net+1,deadline),order(net,0),trade])
                if ([r["status"] for r in (high,expired,paid)]!=["0x0","0x0","0x1"] or settled!=observed
                    or [token_flow(paid["logs"],t,OWNER) for t in path]!=settled):
                    raise ValueError(("limits or funds",mode,path,amount,net))
                rows.append(dict(mode=mode,markets=markets,path=path,spent=str(amount),received=str(net),
                                 gas=int(paid["gasUsed"],16),approval_gas=grant,calldata=trade["data"],
                                 logs=paid["logs"],expired=expired.get("error"),high=high.get("error")))
    return rows

