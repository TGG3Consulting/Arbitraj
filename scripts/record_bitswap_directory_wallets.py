"""BitSwap. Исполнение кошелька по всему реестру Ethereum с измерением собственного финансирования и выплаты."""
import argparse
from collections import deque
import json
from pathlib import Path

import requests
from eth_abi import decode
from record_bitswap_ethereum import (ENDPOINTS, EXEC, FUNDING, OWNER, STABLES, STEP, VIRTUAL, WETH,
                                     balance, environment, simulate_block, tx)


def swap(pool, source, target, amount, minimum, deadline):
    types = ["address", "uint256", "address", "uint256", "uint256", STEP+"[]"]
    params = [source, amount, target, minimum, deadline, [(14, pool, source, target, 10000, b"")]]
    return tx(EXEC, "execute("+",".join(types)+")", types, params)


def allowance(token, amount):
    return tx(token, "approve(address,uint256)", ["address", "uint256"], [EXEC, amount])


def funding_paths(markets, target, excluded):
    # Prefer a source which can be acquired without moving the measured pool. At most two BitSwap legs.
    queue = deque((a, ()) for a in (*reversed(STABLES), WETH))
    found = []
    while queue:
        token, legs = queue.popleft()
        if token == target:
            found.append(legs)
            continue
        if len(legs) < 2:
            for m in markets:
                if token not in m["tokens"] or m["pool"] in {pair for _, _, pair in legs}:
                    continue
                other = next(a for a in m["tokens"] if a != token)
                if other in [a for a, _, _ in legs]:
                    continue
                queue.append((other, legs + ((token, other, m["pool"]),)))
    return sorted(found, key=lambda legs:(any(pool == excluded for _, _, pool in legs), len(legs)))


def directory(url):
    j, tag, header, deadline, overrides = environment(url)
    run = lambda calls: simulate_block(url, tag, overrides, calls)
    markets = j["markets"]
    funded = {}
    def prepare(token, path):
        key = (token, path)
        if key in funded:
            return funded[key]
        seed = path[0][0] if path else token
        amount = 10**16 if seed == WETH else 10**7
        seed_call = (tx(WETH, "deposit()", value=amount) if seed == WETH else
                     tx(FUNDING, "swapETHForExactTokens(uint256,address[],address,uint256)",
                        ["uint256","address[]","address","uint256"],[amount,[WETH,seed],OWNER,deadline],10**18))
        prefix = [seed_call]
        for a, b, pool in path:
            next_leg = [allowance(a, amount), swap(pool, a, b, amount, 1, deadline)]
            rows = run(prefix + [balance(b, OWNER)] + next_leg + [balance(b, OWNER)])
            prefix.extend(next_leg)
            if any(row["status"] != "0x1" for row in rows):
                funded[key] = None
                return None
            amount = int(rows[-1]["returnData"],16) - int(rows[-4]["returnData"],16)
            if amount < 2:
                funded[key] = None
                return None
        funded[key] = (prefix, amount)
        return funded[key]

    def outside(token):
        key = (token, "public-router")
        if key in funded:
            return funded[key]
        # Exact-input public route bounds the actual ETH spent. Read the payer change
        # across the funding call, so a pre-existing token balance cannot fund the test.
        for bridge in (None,*STABLES):
            path = [WETH,token] if bridge is None else [WETH,bridge,token]
            transfer = tx(FUNDING,"swapExactETHForTokens(uint256,address[],address,uint256)",
                          ["uint256","address[]","address","uint256"],[1,path,OWNER,deadline],10**16)
            before, action, after = run([balance(token,OWNER),transfer,balance(token,OWNER)])
            if all(v["status"]=="0x1" for v in (before,action,after)):
                acquired=int(after["returnData"],16)-int(before["returnData"],16)
                if acquired >= 2:
                    funded[key]=(([transfer],acquired),[(a,b,FUNDING) for a,b in zip(path,path[1:])])
                    return funded[key]
        funded[key]=None
        return None

    cases, unavailable = [], []
    for m in markets:
        for a in m["tokens"]:
            b = next(t for t in m["tokens"] if t != a)
            candidates = funding_paths(markets,a,m["pool"])
            account = None
            for path in candidates:
                if any(pool==m["pool"] for _,_,pool in path):
                    break
                account = prepare(a,path)
                if account is not None:
                    break
            if account is None:
                external = outside(a)
                if external is not None:
                    account, path = external
                else:
                    for path in candidates:
                        if any(pool==m["pool"] for _,_,pool in path):
                            account = prepare(a,path)
                            if account is not None:
                                break
            if account is None:
                unavailable.append(dict(pool=m["pool"],source=a,phase="funding",routes_tested=len(candidates),
                                        external_router=FUNDING,external_bridges=[WETH,*STABLES]))
                continue
            prefix, wallet = account
            for amount in sorted({wallet//10, wallet//2} - {0}):
                pre = prefix + [allowance(a, amount)]
                balances = [balance(t,w) for w in (OWNER,EXEC) for t in (a,b)]
                place = lambda minimum,until: swap(m["pool"],a,b,amount,minimum,until)
                q = tx(VIRTUAL,"quoteIzi(address,bool,uint256)",["address","bool","uint256"],[m["pool"],m["tokens"][0]==a,amount])
                trades = run(pre+balances+[q,place(1,deadline)]+balances)
                n = len(pre)
                if any(r["status"]!="0x1" for r in trades[:n]):
                    raise ValueError(("funding changed",m["pool"],a,amount))
                quote, actual = trades[n+4:n+6]
                if quote["status"]!="0x1" or actual["status"]!="0x1":
                    unavailable.append(dict(pool=m["pool"],source=a,amount=str(amount),phase="quote/order",
                                            path=path,quote=quote.get("error"),order=actual.get("error")))
                    continue
                frame = lambda rows:[int(r["returnData"],16) for r in rows]
                old,new = frame(trades[n:n+4]), frame(trades[-4:])
                spent, received = old[0]-new[0],new[1]-old[1]
                if not 0<spent<=amount or received<=0 or new[2:]!=old[2:]:
                    raise ValueError(("wallet or V5 residue",m["pool"],a,amount,old,new))
                checked = run(pre+balances+[place(received+1,deadline),place(received,1)]+balances+[place(received,deadline)]+balances)
                if [i for i,row in enumerate(checked) if row["status"]!="0x1"]!=[n+4,n+5]:
                    raise ValueError(("strict payout or deadline",m["pool"],a,amount))
                if frame(checked[n:n+4])!=old or frame(checked[n+6:n+10])!=old or frame(checked[-4:])!=new:
                    raise ValueError(("balance after rollback or settlement",m["pool"],a))
                used,output=decode(["uint256","uint256"],bytes.fromhex(quote["returnData"][2:]))
                cases.append(dict(pool=m["pool"],source=a,target=b,funding=path,authorized=str(amount),
                                  quote=[str(used),str(output)],spent=str(spent),received=str(received),
                                  gas=int(checked[n+10]["gasUsed"],16),before=list(map(str,old)),after=list(map(str,new)),
                                  logs=checked[n+10]["logs"],strict=checked[n+4]["error"],expired=checked[n+5]["error"]))
    return dict(selector=tag,state_root=header["stateRoot"],pools=len(markets),owner=OWNER,
                sizes="one tenth and one half of acquired source",cases=cases,unavailable=unavailable)


if __name__ == "__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path)
    errors=[]
    dest=p.parse_args().output
    for ep in ENDPOINTS:
        try:
            document=directory(ep)
            dest.write_text(json.dumps(document,separators=(",",":"))+"\n",encoding="utf8")
            break
        except (ValueError, KeyError, RuntimeError, requests.RequestException) as exc:
            errors.append(str(exc)[:480])
    else:
        raise RuntimeError(errors)
