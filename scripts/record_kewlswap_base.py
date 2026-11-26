"""Сверяет фактические CREATE-адреса фабрики KEWL и выплаты из общего с Router02 EIP-2535."""
from eth_utils import keccak
from record_base_slipstream_executor import rpc
from record_diamondswap_base import WETH, direct_wallet, facets


def create_address(factory, nonce):
    number=nonce.to_bytes((nonce.bit_length()+7)//8,"big")
    encoded=number if len(number)==1 and nonce<128 else bytes([0x80+len(number)])+number
    payload=bytes([0x94])+bytes.fromhex(factory[2:])+encoded
    return "0x"+keccak(bytes([0xc0+len(payload)])+payload)[-20:].hex()


def create_directory(c, factory):
    count=int(rpc(c.endpoint,"eth_getTransactionCount",[factory,c.selector]),16)
    if not 1<count<=100000:raise ValueError("unexpected historical creator counter")
    return count,{create_address(factory,n):n for n in range(1,count)}


def wallet(c, directory, header):
    proxy=directory["factory"]
    creator=facets(c,proxy,["allPairsLength()","allPairs(uint256)","getPair(address,address)","createPair(address,address)"])
    router=facets(c,proxy,["getAmountsOut(uint256,address[])","getAmountsIn(uint256,address[])",
                       "swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)"])
    code=bytes.fromhex(rpc(c.endpoint,"eth_getCode",[next(iter(creator["implementations"])),c.selector])[2:])
    if code[0xd57:0xd61]!=bytes.fromhex("604051809103906000f0"):
        raise ValueError("CREATE bytecode changed")
    for field in ("factory()","WETH()"):
        selector=keccak(text=field)[:4]
        facet,=c.one(proxy,"facetAddress(bytes4)",["bytes4"],[selector],["address"])
        if int(facet,16):raise ValueError("unexpected separate router binding")
    cases=[]
    for p in directory["pools"]:
        p["balances"]=[str(c.one(t,"balanceOf(address)",["address"],[p["pool"]],["uint256"])[0]) for t in p["tokens"]]
        if WETH not in p["tokens"] or p["reserves"]!=p["balances"]:continue
        reserve=int(p["reserves"][p["tokens"].index(WETH)])
        if reserve<10000 or not min(map(int,p["reserves"])):continue
        sizes=[reserve//10000,reserve//1000]
        row={"pool":p["pool"]}
        try:
            row["legs"]=direct_wallet(c,p,sizes,int(header["timestamp"],16),3000,proxy)
        except ValueError as err:
            row["reason"]=str(err)[:240]
        cases.append(row)
    if not any(len(k.get("legs",[]))==4 for k in cases):
        raise ValueError("no market with full KEWL wallet controls "+str(cases)[:300])
    return dict(creator=creator,router=router,create_pc=0xd60,child_tx_nonce=directory["origin"]["account_nonce"],markets=cases)
