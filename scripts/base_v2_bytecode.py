"""Первичные свидетельства для фабрик, исходный Solidity которых не опубликован в Sourcify."""
from eth_utils import keccak

from record_base_slipstream_executor import rpc

# Адрес пары, хеши реально исполняемого кода фабрики и пары, начало runtime и PUSH2 знаменателя в swap.
PAIR_CODE = {
    "pixelswap-base": ("0x1e9cb689c04324b1b7f63c6e40e2b6b4b4a6e6bc",
        "3043726025a029f9da3b255cf6cdec9c8c240f59cdcba7799fa4bac9df84b35b",
        "0e90d16685aa7f77c63f2a2b8d9392530f6e22dc2b214c2630f19a0a34842240",2005,0x9e6),
    "satori-base": ("0xebd231ee6854517d228aff9266b7068a784dc15f",
        "043d2849408a7faae3d2e674b482eb62df76e920009e776657c238d7400848e5",
        "ed93a8e950a7beb7a796ea08c228357fbb1074528679b7d1245f5a2c15da671a",2433,0xc32),
    "candyswap": ("0x87c7ebbea5301c7482764395470e0f139d189623",
        "335f28a2c41d864d94b9d3059ddba10b11cdda6fa8518a3ba5dfbd43ff8d8b18",
        "3d594794412e87b15488124594b7bb35ed2be3adc175a7773065525ec5079963",2442,0xa9f),
}


def proof(name,c,row):
    factory,den,fee,_,offset,length=row
    pair,fh,ph,runtime_at,pc=PAIR_CODE[name]
    factory_code,pair_code=[bytes.fromhex(rpc(c.endpoint,"eth_getCode",[addr,c.selector])[2:]) for addr in (factory,pair)]
    if keccak(factory_code).hex()!=fh or keccak(pair_code).hex()!=ph:
        raise ValueError("the recorded deployment no longer matches the archived EVM code")
    creation=factory_code[offset:offset+length]
    if (runtime_at<offset or not creation.startswith(bytes.fromhex("6080604052")) or
            factory_code[runtime_at:runtime_at+len(pair_code)]!=pair_code or runtime_at+len(pair_code)>offset+length):
        raise ValueError("the complete pair is not embedded in factory creation code")
    if pair_code[pc:pc+3]!=bytes([0x61])+den.to_bytes(2,"big"):
        raise ValueError("invariant denominator at the verified instruction differs")
    # Статические пары вычитают amountIn * 3 в обеих ногах. У CandySwap ставка загружается
    # из slot 13, младшего uint32. Не путать её с независимым получателем протокольной комиссии.
    if fee is not None and (pair_code[pc-17:pc-15]!=bytes([0x60,fee]) or
                            bytes([0x84,0x60,fee]) not in pair_code[pc+3:pc+53]):
        raise ValueError("one of the two swap input fee multipliers differs")
    if fee is None and bytes.fromhex("600d5463ffffffff") not in pair_code[pc-65:pc]:
        raise ValueError("CandySwap fee storage slot or bit mask differs")
    init=keccak(creation)
    result=dict(url="https://basescan.org/address/"+factory+"#code",runtime="0x"+fh,pair_runtime="0x"+ph,
                pair=pair,creation_code_hash="0x"+init.hex(),creation_offset=offset,creation_bytes=length,
                pair_runtime_offset=runtime_at,denominator_pc=pc,invariant_bytes=pair_code[pc-52:pc+120].hex(),
                provenance="canonical EVM bytecode and wallet replay",fee_numerator=fee,fee_denominator=den)
    if name!="satori-base":
        if c.one(factory,"INIT_CODE_PAIR_HASH()",returns=["bytes32"])!=(init,):
            raise ValueError("factory's public getter differs from the extracted creation code")
    if fee is None:
        result.update(fee_getter="swapFee()",fee_target="pool",fee_slot=13)
    return result

def check_deployments(c, directory):
    """Сверяет полный runtime каждой пары, для изменяемой комиссии также источник текущего значения."""
    proof=directory["origin"]
    for market in directory["pools"]:
        pool=market["pool"]
        code=rpc(c.endpoint,"eth_getCode",[pool,c.selector])
        if "0x"+keccak(bytes.fromhex(code[2:])).hex()!=proof["pair_runtime"]:
            raise ValueError("a registered pair has different swap bytecode: "+pool)
        if proof.get("fee_target")=="pool":
            getter,=c.one(pool,proof["fee_getter"],returns=["uint256"])
            response=c.session.post(c.endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_getStorageAt",
                "params":[pool,hex(proof["fee_slot"]),c.selector]},timeout=20)
            response.raise_for_status()
            word=response.json().get("result")
            if not word or int(word,16)&0xffffffff!=getter:
                raise ValueError("fee getter does not read the word used in the invariant")
            market["commission"]={"getter":proof["fee_getter"],"value":getter,"denominator":proof["fee_denominator"]}
