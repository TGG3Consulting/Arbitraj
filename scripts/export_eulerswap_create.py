"""Неподписанный CREATE для двух официальных фабрик одной сети. Проверяется против оплаченных выплат."""
import argparse
import gzip
import json
from hashlib import sha256
from pathlib import Path
from eth_abi import encode
from eth_utils import keccak

from record_eulerswap_directory import DATA,Reader
from record_base_slipstream_executor import OWNER


def capture(network, artifact):
    reader=Reader(network)
    wallet=json.loads(gzip.decompress((DATA/f"eulerswap_{network}_wallets.json.gz").read_bytes()))
    registry=json.loads(gzip.decompress((DATA/f"eulerswap_{network}_registry.json.gz").read_bytes()))
    chain=registry["chain"]
    assert wallet["chain"]==chain and reader.tag==registry["selector"]==wallet["selector"]
    roles=[registry["roles"][f"V{v}Factory"] for v in (1,2)]
    for role in roles:
        assert reader.code_hash(role["address"])==role["runtime"]
    factories=[x["address"] for x in roles]
    creation=bytes.fromhex(artifact["creation"])+encode(["address","address"],factories)
    deploy={"from":OWNER,"data":"0x"+creation.hex()}
    runtime=bytes.fromhex(reader.rpc("eth_call",[deploy,reader.tag])[2:])
    assert "0x"+keccak(runtime).hex()==wallet["runtime"]
    # Оценка исполняет конструктор на закреплённом родителе без списания средств.
    gas=int(reader.rpc("eth_estimateGas",[deploy,reader.tag]),16)
    return {"chainId":chain,"kind":"eulerswap","factories":roles,"blockHash":reader.tag["blockHash"],
            "runtimeKeccak":wallet["runtime"],"runtimeSha256":sha256(runtime).hexdigest(),"compiler":artifact["solc"],
            "creationSha256":sha256(creation).hexdigest(),"archiveGas":gas,"data":deploy["data"]}


if __name__=="__main__":
    ap=argparse.ArgumentParser()
    ap.add_argument("network",choices=["ethereum","base"])
    ap.add_argument("artifact",type=Path)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    args.output.write_text(json.dumps(capture(args.network,json.loads(args.artifact.read_bytes())),separators=(",",":")))

