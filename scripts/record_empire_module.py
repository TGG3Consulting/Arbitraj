"""Проверяет код при реальном CREATE и один защищённый шаг существующего V5 в изолированной симуляции."""
from eth_abi import encode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, OWNER, STEP, call, calldata, rpc
from record_stabull_wallets import request

MODULE = "0x"+keccak(b"\xd6\x94"+bytes.fromhex(OWNER[2:])+b"\x80")[-20:].hex()


def deployment(endpoint,tag,factory,artifact):
    if int(rpc(endpoint,"eth_getTransactionCount",[OWNER,tag]),16) or rpc(endpoint,"eth_getCode",[MODULE,tag])!="0x":
        raise ValueError("isolated creator nonce/address used")
    role=encode(["address"],[factory])
    creation={"from":OWNER,"data":"0x"+artifact["creation"]+role.hex(),"gas":hex(3000000)}
    code=bytes.fromhex(request(endpoint,"eth_call",[creation,tag])[2:])
    expected=bytes.fromhex(artifact["runtime"])
    masked=bytearray(code)
    [refs]=artifact["immutableReferences"].values()
    for field in refs:
        i,n=field["start"],field["length"]
        if n!=32 or code[i:i+n]!=role:
            raise ValueError("wrong immutable factory")
        masked[i:i+n]=expected[i:i+n]
    if masked!=expected or not refs:
        raise ValueError("unexpected deployed code")
    return creation,dict(runtime="0x"+keccak(code).hex(),factory=factory,bytes=len(code),solc=artifact["solc"])


def exchange(mode,pool,tokens,amount,minimum,deadline):
    source,target=tokens
    if mode=="module":
        return call(MODULE,calldata("execute(address,address,address,uint256,uint256,uint256)",
            ["address","address","address","uint256","uint256","uint256"],[pool,source,target,amount,minimum,deadline]))
    return call(EXEC,calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
        ["address","uint256","address","uint256","uint256",STEP+"[]"],
        [source,amount,target,minimum,deadline,[(29,MODULE,source,target,10000,b"")]]))
