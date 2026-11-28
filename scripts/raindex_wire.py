"""Публичный ABI takeOrders пяти поколений. Вход maker и расход taker имеют разные единицы."""
from decimal import Decimal, localcontext
from eth_abi import decode
from record_raindex_quotes import ORDER, data

ORDERS={
    3:"(address,bool,(address,address,address),(address,uint8,uint256)[],(address,uint8,uint256)[])",
    4:"(address,(address,address,bytes),(address,uint8,uint256)[],(address,uint8,uint256)[],bytes32)",
    5:ORDER,6:ORDER,7:ORDER,
}
METHODS={3:"takeOrders",4:"takeOrders2",5:"takeOrders3",6:"takeOrders4",7:"takeOrders5"}
SELECTORS={3:"8a44689c",4:"0997c4a0",5:"d0f7a862",6:"69c72856",7:"b0460f07"}


def float_raw(amount,decimals):
    if not 0<=amount<1<<223 or not 0<=decimals<=255:
        raise ValueError("float precision")
    return (-decimals).to_bytes(4,"big",signed=True)+amount.to_bytes(28,"big")


def decimal(word):
    b=bytes.fromhex(word.removeprefix("0x"))
    return Decimal(int.from_bytes(b[4:],"big",signed=True)).scaleb(int.from_bytes(b[:4],"big",signed=True))


def take(version,order,i,j,minimum,maximum,decimals,receive_fixed,ratio_limit=None):
    if version not in (6,7) and not receive_fixed:
        raise ValueError("generation limits output purchased from maker")
    t=ORDERS[version]
    signed={3:"(address,uint256[],bytes)",4:"(address,uint256[],bytes)",5:"(address,bytes32[],bytes)",6:"(address,bytes32[],bytes)",7:"(address,bytes32,bytes32[],bytes)"}[version]
    entry=f"({t},uint256,uint256,{signed}[])"
    o=decode([t],bytes.fromhex(order.removeprefix("0x")))[0]
    c=(o,i,j,[])
    if version<=4:
        spec=f"(uint256,uint256,uint256,{entry}[],bytes)"
        values=(minimum,maximum,(1<<256)-1,[c],b"")
    else:
        flag=",bool" if version>=6 else ""
        spec=f"(bytes32,bytes32,bytes32{flag},{entry}[],bytes)"
        values=(float_raw(minimum,decimals),float_raw(maximum,decimals),ratio_limit if ratio_limit is not None else float_raw(10**50,0),*((receive_fixed,) if version>=6 else ()),[c],b"")
    action=data(METHODS[version],[spec],[values])
    if action[2:10]!=SELECTORS[version]:raise ValueError("ABI mismatch")
    return action
