"""Сверка двух точных takeOrders4 на одном зафиксированном блоке с оплатой собственного модуля."""
import gzip
import json
from eth_abi import decode
from record_raindex_quotes import DATA, TAKE, ORDER, data
from record_base_slipstream_executor import OWNER, call
from raindex_wire import take


def comparison(src, module, tag, stamp, seed, run, bal, approve, tx, end, identities):
    wallet = json.loads(gzip.decompress((DATA/"raindex_base_protected_wallets.json.gz").read_bytes()))
    if wallet["selector"] != tag or wallet["payer"] != OWNER or wallet["stamp"] != stamp:
        raise ValueError("original wallet and head")
    entries = {e["hash"]: e for e in src["entries"]}
    book = src["book"]
    limits = "(address,address,uint256,uint256,uint256,uint256)"
    cases = []
    for pair in wallet["pairs"]:
        paid = pair.get("partial_capacity", {})
        if paid.get("stage") != "paid":
            continue
        h, i, j = paid["order"], paid["i"], paid["j"]
        order = entries[h]["order"]
        pay, receive = paid["pay"], paid["receive"]
        quote = call(book, data("quote2", [TAKE], [(decode([ORDER], bytes.fromhex(order[2:]))[0], i, j, [])]))
        quote["from"] = module
        ops = run([quote, tx(pay,"decimals",[],[]), tx(receive,"decimals",[],[])])
        if any(op["status"] != "0x1" for op in ops):
            raise ValueError("module quote or token decimals")
        live, cap, ratio = decode(["bool","bytes32","bytes32"], bytes.fromhex(ops[0]["returnData"][2:]))
        if not live:
            raise ValueError("inactive quote from module")
        di, dout = (int(op["returnData"],16) for op in ops[1:])
        spent, net = int(paid["paid"]), int(paid["net"])
        checks = [bal(pay), bal(receive), bal(pay,module), bal(receive,module),
                  tx(pay,"allowance",["address","address"],[module,book])]
        result = dict(order=h, i=i, j=j, pay=pay, receive=receive, decimals=[di,dout],
                      outputMax="0x"+cap.hex(), ratio="0x"+ratio.hex(), target=module,
                      spent=str(spent), net=str(net))
        for key, quantity, decimals, receive_fixed in [
            ("exactDebit", spent, di, False), ("exactReceive", net, dout, True),
        ]:
            action = take(6, order, i, j, quantity, quantity, decimals, receive_fixed, ratio)
            payment = tx(module,"fill",[limits,"bytes"],
                         [(pay, receive, spent, spent, net, end),bytes.fromhex(action[2:])])
            prefix = seed + [approve(pay,module,spent)]
            rows = run(prefix + checks + [payment] + checks)
            pos = len(prefix) + len(checks)
            if any(op["status"] != "0x1" for op in rows[:pos]+rows[pos+1:]):
                raise ValueError("funding or balances")
            op = rows[pos]
            snapshot = lambda section: [str(int(v["returnData"],16)) for v in section]
            result[key] = dict(action=action, call=payment, status=op["status"],
                               before=snapshot(rows[len(prefix):pos]), after=snapshot(rows[pos+1:]),
                               logs=op["logs"], returnData=op.get("returnData"), error=op.get("error"))
        cases.append(result)
    if len(cases) != 7:
        raise ValueError("seven recorded capacity edges expected")
    return dict(chain=8453,height=src["height"],selector=tag,stamp=stamp,book=book,payer=OWNER,
                modules=identities, cases=cases)
