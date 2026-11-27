"""Verify Base classic clone factories, both registry directions and wallet-owned swaps at one block."""
import argparse
import json
from pathlib import Path

from eth_utils import keccak
from fork_fee_rpc import Reader
from record_llama_base_cl import CanonicalCalls, ROOT, WETH
from clone_classic_wallets import measure

VENUES = {
    "virtus-amm": ("0x7f03ae4452192b0e280fb0d4f9c225dda88c7623", "0xd08270b8149dbde478da8adad979246e957be866"),
    "hydrometer": ("0xf60cacf0a3daa5b6a79ca6594bef38f85391ae0a", "0xed6d68b9048a1842e31ab41ebfd1262e501fbea5"),
}


def directory(name, reader, cap):
    factory, router = VENUES[name]
    def one(to, sig, result, types=(), values=()):
        return cap.one(to,sig,types,values,[result])[0]
    assert one(router,"defaultFactory()","address")==factory
    implementation = one(factory,"implementation()","address")
    registry = one(router,"factoryRegistry()","address")
    assert one(router,"weth()","address")==WETH
    assert one(registry,"isPoolFactoryApproved(address)","bool",["address"],[factory])
    addresses=dict(factory=factory,router=router,implementation=implementation,registry=registry)
    roles={}
    for key,a in addresses.items():
        code = bytes.fromhex(reader.one("eth_getCode",[a,cap.selector])[2:])
        assert code
        roles[key]=dict(address=a,bytes=len(code),runtime="0x"+keccak(code).hex())
    runtime=bytes.fromhex("363d3d373d3d3d363d73"+implementation[2:]+"5af43d82803e903d91602b57fd5bf3")
    init=keccak(bytes.fromhex("3d602d80600a3d3981f3")+runtime)
    length=one(factory,"allPoolsLength()","uint256")
    entries=[]
    for i in range(length):
        pool=one(factory,"allPools(uint256)","address",["uint256"],[i])
        a,b=(one(pool,"token"+str(j)+"()","address") for j in range(2))
        stable=one(pool,"stable()","bool")
        assert int(a,16)<int(b,16)
        assert one(pool,"factory()","address")==factory
        assert one(factory,"isPool(address)","bool",["address"],[pool])
        salt=keccak(bytes.fromhex(a[2:]+b[2:])+bytes([stable]))
        assert pool=="0x"+keccak(b"\xff"+bytes.fromhex(factory[2:])+salt+init)[-20:].hex()
        assert reader.one("eth_getCode",[pool,cap.selector])=="0x"+runtime.hex()
        for x,y in [(a,b),(b,a)]:
            assert one(factory,"getPool(address,address,bool)","address",["address","address","bool"],[x,y,stable])==pool
            assert one(router,"poolFor(address,address,bool,address)","address",["address","address","bool","address"],[x,y,stable,factory])==pool
        reserves=cap.one(pool,"getReserves()",returns=["uint256"]*3)[:2]
        balances=[one(t,"balanceOf(address)","uint256",["address"],[pool]) for t in (a,b)]
        fee=one(factory,"getFee(address,bool)","uint256",["address","bool"],[pool,stable])
        custom=one(factory,"customFee(address)","uint256",["address"],[pool])
        base=one(factory,("stableFee" if stable else "volatileFee")+"()","uint256")
        zero=one(factory,"ZERO_FEE_INDICATOR()","uint256")
        assert fee==(0 if custom==zero else custom or base)
        entries.append(dict(pool=pool,tokens=[a,b],stable=stable,custom_fee=custom,base_fee=base,fee=fee,
                            reserves=list(map(str,reserves)),balances=list(map(str,balances))))
    return dict(name=name,chain_id=8453,selector=cap.selector,height=int(reader.header["number"],16),
                roles=roles,init="0x"+init.hex(),paused=one(factory,"isPaused()","bool"),markets=entries)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("name",choices=VENUES)
    p.add_argument("output",type=Path)
    args=p.parse_args()
    head=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    reader=Reader("base",head)
    cap=CanonicalCalls(reader)
    args.output.write_bytes(json.dumps(measure(reader,cap,directory(args.name,reader,cap)),separators=(",",":")).encode())

