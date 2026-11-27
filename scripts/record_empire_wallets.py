"""Измеряет обе формы EmpireRouter и V5 на каждой зарегистрированной паре Ethereum."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode

from record_balancer_markets import Capture
from record_empire_module import MODULE, deployment, exchange
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, rpc, simulate_block
from record_ethereum_v2_factories import WETH, digest, raw
from record_ethervista_wallets import integer, provenance, tx
from record_v2_chain_pools import swap

ROUTER = "0xe7a504316bebbe540496e29798187c9ecad6ef4f"
SOURCE = ROOT / "tests/data/first_run_dex_2026-11-26/llama_eth_v2_wallets.json.gz"
ZERO = "0x" + "00" * 20


def observations(pool, tokens):
    views = [tx(t, "balanceOf(address)", ["address"], [a])
             for a in (OWNER, pool, EXEC) for t in tokens]
    return views + [tx(pool, "sweptAmount()"), tx(pool, "getReserves()")]


def state(rows):
    values = [integer(r) for r in rows[:-1]]
    reserves = decode(["uint112", "uint112", "uint32"], raw(rows[-1]["returnData"]))[:2]
    return dict(owner=values[:2], physical=values[2:4], executor=values[4:6],
                debt=values[6], reserves=list(reserves))


def action(mode, pool, tokens, amount, minimum, deadline):
    if mode in ("module","v5module"):
        return exchange(mode,pool,tokens,amount,minimum,deadline)
    if mode == "v5":
        return swap(pool, *tokens, amount, minimum, deadline, 3000)
    sig = "swapExactTokensForTokens" + ("SupportingFeeOnTransferTokens" if mode == "fot" else "")
    return tx(ROUTER, sig+"(uint256,uint256,address[],address,uint256)",
              ["uint256","uint256","address[]","address","uint256"],
              [amount,minimum,tokens,OWNER,deadline])


def leg(sim, prefix, pool, tokens, amount, mode, deadline):
    target = EXEC if mode in ("v5","v5module") else MODULE if mode=="module" else ROUTER
    approval = tx(tokens[0],"approve(address,uint256)",["address","uint256"],[target,amount])
    views = observations(pool,tokens)
    def trial(minimum, expiry):
        trade = action(mode,pool,tokens,amount,minimum,expiry)
        calls = sim([*prefix,approval,*views,trade,*views])
        n = len(prefix)+1
        if any(v["status"] != "0x1" for v in calls[:n]):
            raise ValueError("independent funding failed")
        before,after = state(calls[n:n+len(views)]),state(calls[-len(views):])
        paid = before["owner"][0]-after["owner"][0]
        received = after["owner"][1]-before["owner"][1]
        r = calls[n+len(views)]
        if r["status"] != "0x1" and before != after:
            raise ValueError("reverted operation moved funds")
        return dict(status=r["status"],error=r.get("error"),gas=r["gasUsed"],approval_gas=calls[n-1]["gasUsed"],calldata=trade["data"],
                    before=before,after=after,paid=paid,received=received,logs=r.get("logs",[])),trade
    loose,_ = trial(1,deadline)
    if loose["status"] != "0x1" or loose["received"] <= 0:
        return dict(loose=loose), None
    net = loose["received"]
    strict,trade = trial(net,deadline)
    above,_ = trial(net+1,deadline)
    expired,_ = trial(net,1)
    if strict["paid"] != amount or strict["received"] != net or expired["status"] != "0x0":
        raise ValueError("non-repeatable payment or deadline")
    return dict(exact=strict,above=above,expired=expired,net_guard=above["status"]=="0x0"),[*prefix,approval,trade]


def capture(endpoint, doc, artifact):
    c = Capture(dict(endpoint=endpoint,selector=doc["selector"]))
    h = rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if h["stateRoot"]!=doc["state_root"] or int(rpc(endpoint,"eth_chainId",[]),16)!=1:
        raise ValueError("wrong chain or state")
    router = provenance("ethereum",ROUTER,c)
    for role,actual in (("factory()",doc["factory"]),("WETH()",WETH)):
        if c.one(ROUTER,role,returns=["address"])[0]!=actual:
            raise ValueError("official router "+role+" differs")
    deadline = int(h["timestamp"],16)+900
    word=lambda n:"0x"+format(n,"064x")
    env={OWNER:{"balance":hex(10**20)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
         "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    sim=lambda calls:simulate_block(endpoint,c.selector,env,calls)
    modes=("nominal","fot","v5")
    prefix=[]
    identity=None
    if artifact:
        create,identity=deployment(endpoint,c.selector,doc["factory"],artifact)
        [created]=sim([create])
        if created["status"]!="0x1":raise ValueError("CREATE failed")
        identity["creation_gas"]=int(created["gasUsed"],16)
        prefix=[create]
        modes=("module","v5module")
    rows=[]
    for record in doc["pools"]:
        pool,tokens=record["pool"],record["tokens"]
        if c.one(doc["factory"],"getPair(address,address)",["address","address"],tokens,["address"])[0]!=pool:
            raise ValueError("unregistered market")
        other=next(t for t in tokens if t!=WETH)
        sweepable=c.one(pool,"sweepableToken()",returns=["address"])[0]
        initial=state(sim(observations(pool,tokens)))
        if sweepable not in [ZERO,*tokens] or (sweepable==ZERO and initial["debt"]):
            raise ValueError("invalid sweep side")
        surplus={t for i,t in enumerate(tokens) if initial["physical"][i]>initial["reserves"][i]}
        row=dict(pool=pool,tokens=tokens,sweepable=sweepable,initial=initial,samples=[])
        for size in (10**12,10**15):
            deposit=[*prefix,tx(WETH,"deposit()",value=size)]
            purchases={}
            for mode in modes:
                if WETH in surplus and mode in ("v5","fot"):
                    continue
                result,next_prefix=leg(sim,deposit,pool,[WETH,other],size,mode,deadline)
                row["samples"].append(dict(side="buy",input=size,mode=mode,result=result))
                if next_prefix:
                    purchases[mode]=(next_prefix,result["exact"]["received"])
            for mode in modes:
                if not purchases or (other in surplus and mode in ("v5","fot")):
                    continue
                origin=mode if mode in purchases else next(iter(purchases))
                prefix,amount=purchases[origin]
                result,_=leg(sim,prefix,pool,[other,WETH],amount,mode,deadline)
                row["samples"].append(dict(side="sell",input=amount,seed=size,funding=origin,mode=mode,result=result))
        # Обе границы входа измеряются с новым собственным WETH, без обращения к функции владельца sweep/unsweep.
        row["debt_boundary"]=[]
        if initial["debt"] and sweepable==WETH:
            for size in (initial["debt"]-1,initial["debt"]+10**12):
                for mode in modes:
                    funds=[*prefix,tx(WETH,"deposit()",value=size)]
                    result,_=leg(sim,funds,pool,[WETH,other],size,mode,deadline)
                    row["debt_boundary"].append(dict(input=size,mode=mode,result=result))
        rows.append(row)
    if not any(s["result"].get("net_guard") for row in rows for s in row["samples"]):
        raise ValueError("no historical payment with a terminal floor")
    return dict(selector=c.selector,state_root=h["stateRoot"],factory=doc["factory"],router=ROUTER,
                module=identity,router_proof=router,executor_hash=digest(raw(env[EXEC]["code"])),owner=OWNER,pools=rows)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path)
    p.add_argument("--module",type=Path)
    args=p.parse_args()
    with gzip.open(SOURCE) as f:
        doc=json.load(f)["empire"]
    for endpoint in dict.fromkeys((doc["endpoint"],"https://mainnet.gateway.tenderly.co","https://eth.drpc.org")):
        try:
            result=capture(endpoint,doc,json.loads(args.module.read_bytes()) if args.module else None)
            args.output.write_text(json.dumps(result)+"\n",encoding="utf8")
            break
        except (requests.RequestException,ValueError,KeyError) as e:
            print(endpoint,str(e)[:200],flush=True)
    else:
        raise SystemExit("No Ethereum archive completed the Empire payment matrix.")
