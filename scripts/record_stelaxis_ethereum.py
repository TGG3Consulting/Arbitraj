"""Сверяет справочник, обычные котировки и подтверждённую выплату Stelaxis на канонических блоках."""
from pathlib import Path
import argparse
import json

from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import calldata
from record_fermi_ethereum import ROOT, USDT, WETH, block, rpc, selector

WBTC="0x2260fac5e5542a773aa44fbcfedf7c193bc2c599"


def read(venue, at, name, types=(), args=()):
    try:
        return rpc("eth_call", [{"to":venue, "data":calldata(name, types, args)}, at])
    except ValueError as exc:
        detail = exc.args[0]
        if isinstance(detail, tuple) and len(detail)==2 and isinstance(detail[1], dict):
            err=detail[1]
            if err.get("code")==3 and err.get("data"):
                return {"revert":err["data"]}
        raise


def main(output):
    data=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/stelaxis_ethereum.json").read_text())
    venue=data["venue"]
    directory=data["directory"]
    h=block(directory["block"])
    at=selector(h)
    if rpc("eth_chainId",[])!="0x1" or at!=directory["selector"]:
        raise ValueError("другая сеть или блок")
    raw=read(venue,at,"getPairs()")
    pairs=[list(p) for p in decode(["(address,address)[]"],bytes.fromhex(raw[2:]))[0]]
    if pairs!=directory["pairs"]:
        raise ValueError("изменился справочник")
    samples=[]
    for a,b in [direction for p in pairs for direction in (p,p[::-1])]:
        amount=10**16 if a==WETH else 10**4 if a==WBTC else 10**6
        active=read(venue,at,"isActive(address,address)",["address","address"],[a,b])
        quote=read(venue,at,"quote(address,address,uint256)",["address","address","uint256"],[a,b,amount])
        if int(active,16) or quote!={"revert":"0x9c3d724b"}:
            raise ValueError("контрольное направление изменилось")
        samples.append(dict(token_in=a,token_out=b,amount=str(amount),active=active,quote=quote))
    historic=[]
    for n,digest,*_ in data["slot_examples"]:
        old=block(n)
        if old["hash"]!=digest:
            raise ValueError("перестроен блок исторической котировки")
        quoted=read(venue,selector(old),"quote(address,address,uint256)",["address","address","uint256"],[WETH,USDT,10**16])
        historic.append(dict(height=n,block_hash=digest,quote=quoted))
    if not any(isinstance(x["quote"],str) and int(x["quote"],16)>0 for x in historic):
        raise ValueError("нет положительного исторического контроля")
    expected=data["receipt"]
    receipt=rpc("eth_getTransactionReceipt",[expected["transaction"]])
    if receipt["status"]!="0x1" or receipt["blockHash"]!=block(expected["height"])["hash"] or int(receipt["transactionIndex"],16)!=expected["transaction_index"]:
        raise ValueError("не совпала каноническая квитанция")
    logs={int(row["logIndex"],16):row for row in receipt["logs"]}
    event=logs[int(expected["event"]["logIndex"],16)]
    if event["address"]!=venue or (event["topics"],event["data"])!=(expected["event"]["topics"],expected["event"]["data"]):
        raise ValueError("нет соответствующего события Stelaxis")
    transfer="0x"+keccak(text="Transfer(address,address,uint256)").hex()
    for recorded in expected["transfers"]:
        row=logs[recorded["log_index"]]
        if row["topics"][0]!=transfer or row["address"]!=recorded["token"] or str(int(row["data"],16))!=recorded["amount"]:
            raise ValueError("неверные токен или сумма выплаты")
        if ["0x"+t[-40:] for t in row["topics"][1:]]!=[recorded["sender"],recorded["recipient"]]:
            raise ValueError("неверный отправитель или получатель")
    output.write_text(json.dumps(dict(chain_id=1,canonical=at,pairs=pairs,samples=samples,historic=historic,
        transaction=receipt["transactionHash"],transfers=expected["transfers"]),indent=2)+"\n",encoding="utf8")


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    main(parser.parse_args().output)
