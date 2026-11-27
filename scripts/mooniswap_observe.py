"""Снимок входов денежной формулы Mooniswap в том же симулированном блоке."""
from eth_abi import decode
from record_bitswap_ethereum import tx

def frame(pool,pair,successor,multi):
    calls=[tx(multi,"getCurrentBlockTimestamp()"),tx(pool,"fee()"),tx(pool,"decayPeriod()")]
    if successor:
        calls.append(tx(pool,"slippageFee()"))
    for token in pair:
        for fn in ("virtualBalancesForAddition","virtualBalancesForRemoval"):
            calls.append(tx(pool,fn+"(address)",["address"],[token]))
    return calls

def decode_frame(rows,successor):
    if any(x["status"]!="0x1" for x in rows):
        raise ValueError("failed virtual-balance or tariff read")
    n=4 if successor else 3
    scalars=[str(int(x["returnData"],16)) for x in rows[:n]]
    if not successor:
        scalars.append("0")
    virtual=[list(map(str,decode(["uint216","uint40"],bytes.fromhex(x["returnData"][2:])))) for x in rows[n:]]
    return dict(zip(("timestamp","fee","decay","slippage_fee"),scalars,strict=True),addition=virtual[::2],removal=virtual[1::2])
