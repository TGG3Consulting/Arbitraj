"""Читает независимые стороны ElfomoFi на исходном блоке и проверяет случаи исчерпания одной стороны в симуляции."""
import sys
from pathlib import Path
import requests
from base_archive_endpoint import available
from eth_utils import keccak

from record_elfomo_executor import EXEC, FOLDER, calldata, read, rpc, write

USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
WETH = "0x4200000000000000000000000000000000000006"
CBBTC = "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf"


def capture(compiled):
    src = available(read(FOLDER / "elfomofi_base.json"))
    accepted = read(FOLDER / "elfomo_executor_base.json")
    code = compiled["runtime"]
    if "0x" + keccak(bytes.fromhex(code)).hex() != accepted["runtime_keccak"]:
        raise ValueError("Хеш модуля расходится с приёмкой")
    router, vault, tag = src["router"], src["vault"], src["selector"]
    owner = next(c["owner"] for c in src["cases"] if c["token_in"] == USDC)
    rows = []
    for coin in (WETH, CBBTC):
        for disabled in (None, coin, USDC):
            calls, names = [], []
            def add(key, caller, target, data):
                names.append(key)
                calls.append({"from": caller, "to": target, "data": data, "gas": "0x16e360"})
            if disabled is not None:
                add("clear_vault_allowance", vault, disabled, calldata("approve(address,uint256)", ["address","uint256"], [router,0]))
            sizes = []
            for i, amount in enumerate((10**6, 10**7)):
                types = ["address", "address", "uint256"]
                sizing = {"from":EXEC,"to":router,"data":calldata("getAmountIn(address,address,uint256)",types,[coin,USDC,amount])}
                response = requests.post(src["endpoint"],json=dict(jsonrpc="2.0",id=1,method="eth_call",params=[sizing,tag]),timeout=20)
                response.raise_for_status()
                sized = response.json()["result"]
                sizes.append(dict(request=sizing,response=sized,usdc=str(amount)))
                for name, a, b, dx in [("buy",USDC,coin,amount), ("sell",coin,USDC,int(sized,16))]:
                    add(name+str(i), owner, EXEC, calldata("getAmountOut(address,address,uint256)",types,[a,b,dx]))
            stage, = rpc(src["endpoint"], "eth_simulateV1", [{"blockStateCalls":[
                {"stateOverrides":{EXEC:{"code":"0x"+code}},"calls":calls}],"validation":False},tag])
            if stage["parentHash"] != tag["blockHash"]:
                raise ValueError("Неверный канонический блок")
            results = dict(zip(names,stage["calls"],strict=True))
            for key, r in results.items():
                rejected = (disabled == coin and key.startswith("buy")) or (disabled == USDC and key.startswith("sell"))
                if rejected:
                    if r["status"] != "0x0" or r["error"]["data"] != "0x"+keccak(text="ExhaustedVault()")[:4].hex():
                        raise ValueError(f"Не подтверждена граница {coin} {key}")
                elif r["status"] != "0x1" or int(r["returnData"],16)==0:
                    raise ValueError(f"Пропала независимая сторона {coin} {key}")
            rows.append(dict(coin=coin,disabled=disabled,sizes=sizes,calls=calls,results=results))
    return dict(selector=tag,endpoint=src["endpoint"],module=EXEC,runtime_keccak=accepted["runtime_keccak"],rows=rows)


if __name__ == "__main__":
    write(FOLDER/"elfomo_ladders_base.json",capture(read(Path(sys.argv[1]))))
