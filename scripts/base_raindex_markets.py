"""Готовит раздел книги из проверенных ответов официальной книги и кошелька."""
import gzip
import json
from pathlib import Path


def raindex_markets():
    root = Path("tests/data/first_run_dex_2026-11-26")
    def read(network, suffix):
        path = root / f"raindex_{network}_{suffix}.json.gz"
        return json.loads(gzip.decompress(path.read_bytes()))
    out = ["### Raindex, ID 5221", "",
           "На Base в официальном индексаторе активны 506 ордеров одной книги V6. Для выбранного taker без дополнительных подписей положительны 18 ответов по 17 ордерам. На Ethereum проверены четыре поколения. В таблицу внесены версии с фактическим расчётом модуля. Прямой столбец измерен в отдельной симуляции из собственного баланса.", "",
           "| Сеть и блок | Книга и runtime | Runtime своего модуля | Прямые выплаты | Выплаты модуля | Газ модуля |",
           "|---|---|---|---:|---:|---:|"]
    for network, version in [("base", 6), ("ethereum", 4)]:
        q = read(network, "quotes")
        m = read(network, "protected_wallets")
        d = read(network, "wallets")
        book = q if version == 6 else next(b for b in q["books"] if b["version"] == version)
        roles = m["modules"][book["book"]]
        def payments(a):
            return [r for pair in a["pairs"] for case in pair["cases"] for r in case.values()
                    if isinstance(r, dict) and r.get("stage") == "paid" and r["version"] == version]
        a, b = payments(d), payments(m)
        gas = [int(r["gas"]) for r in b]
        out.append(f"| {network} {m['height']}, V{version} | `{book['book']}` · `{book['runtime']}` | `{roles['runtime']}` | {len(a)} | {len(b)} | {min(gas):,}–{max(gas):,} |")
    out.extend(["", "Для `raindex.books` запишите chainId, номер версии, адрес книги, хеш её runtime, адрес собственного модуля, хеш его runtime, тип источника и URL. На Base источник graphql указан в [официальных стратегиях](https://github.com/rainlanguage/rain.strategies/blob/d58fd9d89a040c0ace494f12abf5452776b106d5/settings.yaml). На Ethereum используйте тип blockscout и [публичные журналы V4](https://eth.blockscout.com/api/v2/addresses/0x0ea6d458488d1cf51695e1d6e4744e6fb715d37c/logs). Адрес из симуляции не является размещением. Для работы требуются выбранный владельцем модуль, разрешение на расход, ETH для газа, индивидуальные пределы возраста книги, времени запроса и газа. Торговля изначально выключена. [Реестр размещений](https://github.com/rainlanguage/raindex/blob/0352173773372ab0b8f3f779586bec446d4eca99/deployments/raindex.json).", "",
                "На Base для обратной продажи полученного wtSPYM все 18 проверенных попыток отклонил минимум ордера. На Ethereum защищённая покупка 5 токенов V4 при заданном архивном пределе откатилась. По V3, V5 и V7 положительная выплата модуля не подтверждена. Для торговли каждая сумма и направление требуют отдельной успешной симуляции с актуальным плательщиком, включающей handleIO и чистый прирост баланса. Положительная quote2 может предшествовать отказу в расчёте.", ""])
    return out
