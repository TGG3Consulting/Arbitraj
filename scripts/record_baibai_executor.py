"""Архивная матрица общего модуля BasePmmExecutor для двух рынков BaiBai на GitHub Actions."""
from eth_abi import decode
from eth_utils import keccak

from record_baibai import ENTRY, FILL, FOLDER, USDC, VAULT, abi, available, read
from record_elfomo_executor import EXEC, measure, signature, simulate


def capture(compiled, deadline):
    primary = available(read(FOLDER / "baibai_base.json"))
    secondary = read(FOLDER / "baibai_nvdac_base.json")
    if primary["selector"] != secondary["selector"]:
        raise ValueError("Разные контрольные состояния BaiBai")
    source = {**primary, "router":ENTRY, "vault":VAULT}
    code = compiled["runtime"]
    cases = []
    for snapshot in (primary, secondary):
        for case in snapshot["cases"]:
            a, b, dx, owner = case["token_in"], case["token_out"], int(case["amount"]), case["owner"]
            coin = b if a == USDC else a
            query = abi("quoteFor", ["address","address","uint256","address"], [coin,a,dx,EXEC])
            _, preview = simulate(source, code, owner, [("book_net", ENTRY, query)])
            if preview["book_net"]["status"] != "0x1":
                raise ValueError("Невозможно получить цену для плательщика-модуля")
            net, fee = decode(["uint256","uint256"], bytes.fromhex(preview["book_net"]["returnData"][2:]))

            def verify(logs, entry, payer, token_in, token_out, amount, expected):
                event, = [r for r in logs if r["address"] == entry and r["topics"][0] == signature(FILL)]
                args = decode(["address","uint256","uint256","uint256"], bytes.fromhex(event["data"][2:]))
                if ("0x"+event["topics"][1][-40:], "0x"+event["topics"][3][-40:], args, expected) != (
                        coin, EXEC, (token_in,amount,net,fee), net):
                    raise ValueError("Плательщик, fee или сумма в Fill расходятся с ценой модуля")
                transfer = signature("Transfer(address,address,uint256)")
                credits = [int(r["data"],16) for r in logs if r["address"] == token_out and r["topics"][0] == transfer
                           and "0x"+r["topics"][1][-40:] == VAULT and "0x"+r["topics"][2][-40:] == payer]
                if sum(credits) != net:
                    raise ValueError("Отсутствует точное зачисление выходного токена на кошелёк")

            result = measure(source, code, case, deadline, "quoteBai", verify)
            result.update(expected_fee=str(fee), taker_quote=preview["book_net"])
            cases.append(result)
    if len({(c["token_in"], c["token_out"], c["amount"]) for c in cases}) != 8:
        raise ValueError("Неполная матрица двух рынков BaiBai")
    return dict(chain_id=8453, selector=source["selector"], endpoint=source["endpoint"], module=EXEC,
                runtime_keccak="0x"+keccak(bytes.fromhex(code)).hex(), compiler=compiled["solc"], cases=cases)
