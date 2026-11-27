"""Пять физических Ethereum-пулов Empire и фактические варианты оплаты на архивной высоте."""
import gzip
import json
from pathlib import Path


def empire_markets():
    directory = Path("tests/data/first_run_dex_2026-11-26")
    def read(name):
        with gzip.open(directory / ("empire_ethereum_"+name+".json.gz")) as f:
            return json.load(f)
    raw, protected = read("wallets"), read("protected")
    rows = ["### Empire · Ethereum", "",
        "На блоке 26 494 988 фабрика зарегистрировала пять физических пар. Три профиля описывают способы оплаты одной ликвидности. "
        "Для каждого размера покупка начинается с отдельного депозита WETH, продажа — с фактической выплаты этой покупки. "
        "В таблице указан счётчик оплат со строгим минимумом среди четырёх проб (покупка и продажа по двум размерам). "
        "Плата пары составляет 3 из 1000 единиц входа. Обеспечение проверяется по физическому остатку и отдельному sweptAmount. "
        "Право sweep принадлежит адресу второго токена пары. До высоты снимка записаны девять Swept, ноль Unswept.", "",
        "| Пара | Физический WETH, wei | Долг WETH, wei | FOT · V5 | Обычный Router | EmpireWallet · V5 с модулем |",
        "|---|---:|---:|:---:|---:|:---:|"]
    def count(pool, mode):
        return sum(s["mode"] == mode and s["result"].get("net_guard") is True for s in pool["samples"])
    weth="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
    for r,p in zip(raw["pools"],protected["pools"],strict=True):
        assert r["pool"]==p["pool"]
        initial=r["initial"]
        rows.append(f"| `{r['pool']}` | {initial['physical'][r['tokens'].index(weth)]} | "
                    f"{initial['debt'] if r['sweepable']==weth else '0'} | {count(r,'fot')} · {count(r,'v5')} | "
                    f"{count(r,'nominal')} | {count(p,'module')} · {count(p,'v5module')} |")
    r = protected["pools"][0]
    first = next(s for s in r["samples"] if s["mode"]=="module" and s["side"]=="buy")["result"]["exact"]
    via = next(s for s in r["samples"] if s["mode"]=="v5module" and s["side"]=="buy")["result"]["exact"]
    rows.extend(["", "В двух пулах с долгом по WETH официальный FOT и исходный V5 отклонили депозиты на один wei меньше долга. "
        "При входе долг плюс 10¹² wei они приняли полный депозит и рассчитали выход на 10¹² wei. Защищённый модуль использует собственный оплаченный вход с учётом логического резерва. "
        "На первой паре за 13 190 001 000 000 000 000 wei он выдал 3 381 119 442 353 821 842 604 raw ROOTDEX, FOT и V5 — 487 094 841 124 888. "
        "При продаже любая заявка ограничена физическим остатком. Возврат долга проверен в модели. Между архивным рождением пяти пар и контрольным блоком реальный возврат долга не наблюдался.", "",
        "Обычный Router считает номинальный выход. Для оплаты через него профиль разрешает только конфигурированную пару активов с проверенным неизменяемым точным переводом (WETH/DAI). "
        "На двух покупках отражающего токена отдельная операция перераспределения изменила balanceOf кошелька сверх суммы события Transfer. "
        "Для прямого модуля квитанция связывает исходный хеш, сеть, nonce, calldata, идентификатор пары, оба ERC20, участника и единственные события Swap и Filled. "
        "Повтор одной пары в вычисленном последовательном маршруте разрешён при подтверждении точного перевода обоих активов.", "",
        f"Владелец размещает EmpireWallet в карточке `empire-module-ethereum`. Конструктор принимает только `dex.factory`, значение перевода 0 ETH. "
        f"После финализации кнопка сохраняет адрес в `dex.router` этого профиля. Создание в архиве потребовало {protected['module']['creation_gas']} ед. газа. "
        f"На первой покупке за {first['paid']} wei владелец получил {first['received']} raw ROOTDEX. Прямой модуль потребовал "
        f"{int(first['gas'],16)} ед. газа и {int(first['approval_gas'],16)} за исходное разрешение. Через V5 потребовались "
        f"{int(via['gas'],16)} и {int(via['approval_gas'],16)} соответственно. Адрес размещённого владельцем модуля в пресете пуст.", "",
        "[Исходник пары](https://eth.blockscout.com/address/0x163b890f4892d593945579028ec44ef0f20a4633?tab=contract) · "
        "[Предыдущая сборка](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45990115953) · "
        "[Обеспечение, CREATE и денежные проверки](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45993312046)", ""])
    return rows
