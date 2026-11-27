"""Read both physical markets after consecutive Bundles swaps in one Ethereum simulation."""
from eth_abi import encode
from record_base_slipstream_executor import EXEC, OWNER, STEP
from record_bitswap_ethereum import tx, balance, token_flow


def measure(run, record, adapter, end):
    router = record["router"]
    weth, bun = router["weth"], record["protocol_token"]
    oracle_pool = router["oracle_pair"]
    markets = {m["pool"]: m for m in record["markets"]}
    oracle = markets[oracle_pool]["oracle"]
    choices = []
    for m in markets.values():
        if m["pool"] == oracle_pool or weth not in m["tokens"]:
            continue
        votes = [v for v in record["cases"] if v["pool"] == m["pool"] and v["mode"] == "converted"
                 and v["wire"] == "v5" and v["input"] == weth and v["stage"] == "settled"]
        if votes:
            best = min(votes, key=lambda v:int(v["amount"]))
            choices.append((m, int(best["amount"])))
    if len(choices) < 2:
        raise ValueError("two independently payable ordinary markets required")
    def observe():
        return tx(oracle, "price1Average()")
    def output(m):
        return next(t for t in m["tokens"] if t != weth)
    def swap(m, raw, limit):
        step = (17, adapter, weth, output(m), 10000, encode(["address"], [m["pool"]]))
        return tx(EXEC, "execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],
                  [weth,raw,output(m),limit,end,[step]])
    results = []
    for (left, a), (right, b) in zip(choices, choices[1:]+choices[:1]):
        prefix = [tx(weth,"deposit()",value=a+b),tx(weth,"approve(address,uint256)",["address","uint256"],[EXEC,a+b])]
        def quote(m,n):
            return tx(router["address"],"estimateExactETHForTokens(address,uint16,uint256)",
                      ["address","uint16","uint256"],[output(m),m["fee"],n])
        calls = prefix+[observe(),quote(left,a),quote(right,b),swap(left,a,1),observe(),quote(right,b),swap(right,b,1),observe(),
                        balance(weth,OWNER), balance(output(left),OWNER), balance(output(right),OWNER)]
        rows = run(calls)
        if any(row["status"]!="0x1" for row in rows):
            raise ValueError(("coupled fee market",left["pool"],right["pool"],[(i,r.get("error")) for i,r in enumerate(rows) if r["status"]!="0x1"]))
        v = lambda i:int(rows[i]["returnData"],16)
        l,r=rows[5],rows[8]
        if (v(3),v(7))!=(v(11),v(12)) or v(10)!=0 or v(6)!=v(9):
            raise ValueError("wallet payouts or fee oracle differ from the measured sequence")
        if token_flow(l["logs"],weth,OWNER)!=-a or token_flow(r["logs"],weth,OWNER)!=-b:
            raise ValueError("each swap must debit the same payer exactly once")
        results.append(dict(pools=[left["pool"],right["pool"]],amounts=list(map(str,[a,b])),
                            paid=list(map(str,[v(11),v(12)])),standalone_second=str(v(4)),
                            rates=list(map(str,[v(2),v(6),v(9)]))))
    return results
