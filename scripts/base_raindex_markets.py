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
        partials = [pair["partial_capacity"] for pair in m["pairs"]
                    if pair.get("partial_capacity", {}).get("stage") == "paid"]

        gas = [int(r["gas"]) for r in b]
        out.append(f"| {network} {m['height']}, V{version} | `{book['book']}` · `{book['runtime']}` | `{roles['runtime']}` | {len(a)} | {len(b)} полных и {len(partials)} частичных | {min(gas):,}–{max(gas):,} |")
    out.extend(["", "Для `raindex.books` запишите chainId, номер версии, адрес книги, хеш её runtime, адрес собственного модуля, хеш его runtime, тип источника и URL. На Base источник graphql указан в [официальных стратегиях](https://github.com/rainlanguage/rain.strategies/blob/d58fd9d89a040c0ace494f12abf5452776b106d5/settings.yaml). На Ethereum используйте тип blockscout и [публичные журналы V4](https://eth.blockscout.com/api/v2/addresses/0x0ea6d458488d1cf51695e1d6e4744e6fb715d37c/logs). Адрес из симуляции не является размещением. Для работы требуются выбранный владельцем модуль, разрешение на расход, ETH для газа, индивидуальные пределы возраста книги, времени запроса и газа. Торговля изначально выключена. [Реестр размещений](https://github.com/rainlanguage/raindex/blob/0352173773372ab0b8f3f779586bec446d4eca99/deployments/raindex.json).", "",
                "У семи ордеров Base отдельно подтверждён частичный расчёт при максимуме выше доступной ёмкости. Например, при максимуме 1,5 wtSPYM кошелёк получил 1 wtSPYM, заплатив 91 USDC из разрешённых 100,1 USDC. В подборе короткий доступный размер оценивается отдельно. На Base для обратной продажи полученного wtSPYM все 18 проверенных попыток отклонил минимум ордера. На Ethereum подписанный предел 0,5 WETH превышал доступные 0,1 WETH. При ограничении расчётной стоимостью те же 5 токенов V4 получены за 0,00103953273698 WETH. Этот ETH-рынок требует WETH как платёжную валюту. При стандартной настройке USDT прямой поиск пропускает пару. Дополнительный RaindexCashWallet проводит обмен USDT/WETH и оплату ордера в одной транзакции; его адрес выбирает владелец. В двух funded V7 ордерах Ethereum механизм Merkle claim требует незаявленного права конкретного counterparty; денежная цена равна нулю. Для V5 и других V7 размещений полная выплата обычного спотового ордера этим архивом не подтверждена. Для торговли каждая сумма и направление требуют отдельной успешной симуляции с актуальным плательщиком, включающей handleIO и чистый прирост баланса. Положительная quote2 может предшествовать отказу в расчёте.", ""])
    legacy=read("v3","ethereum")
    meta=next(iter(legacy["modules"].values()))
    out.extend([f"Ethereum V3: книга `{meta['book']}`, хеш доверенного модуля `{meta['runtime']}`. BJ/USDT на блоке {legacy['height']}: 80 BJ отклонено; 84, 85, 100 и 200 BJ получены за 504, 510, 600 и 1200 USDT. Выражение maker требует расхода минимум 0,1 транша базой 5000 USDT. Внешнего метода quote у V3 нет. В `raindex.sample_amounts` укажите строки `chainId book input output maxDebit exactCredit` в сырых единицах. Проверка пользуется средствами своего кошелька; простая интерполяция не разрешает платёж без подтверждения на точном размере.", ""])
    routed=read("cash", "ethereum")
    largest=max(int(v["received"]) for v in routed["cases"])
    top=[x for x in routed["cases"] if int(x["received"])==largest]
    out.extend([f"На Ethereum составная оплата проведена в {len(routed['cases'])} изолированных случаях. Например, фактические расходы на получение 5 токенов: {int(top[0]['spent'])/10**6:.6f} USDT и {top[0]['gas']} gas. С 10% запасом промежуточного WETH его остаток возвращён в USDT тем же Router: {int(top[1]['spent'])/10**6:.6f} USDT и {top[1]['gas']} gas. Имя актива определяется контрактом `{top[0]['asset']}`. Подписывается предельное списание USDT и минимальная выдача актива. Настройка `raindex.cash_routes` принимает реальное размещение с хешем `{routed['wrapperCode']}`, книга V4, chainId 1 и процент комиссии 0.3. Измерения обратного ордера на тот же токен отсутствуют.", ""])
    out.extend(["В круге с конверсией USDC/USDT предварительная оценка учитывает действующее разрешение того же кошелька. Для денежной ноги требуются фактические средства, симуляция всего объёма и один канал одиночной транзакции без чаевых. После подписания в журнале этой ноги сохраняются исходный хеш, nonce и параметры обоих активов. При восстановлении из канонической квитанции сверяются ордер и чистая сумма расчётного токена. Исполнимость полного круга зависит также от встречного рынка и доступности ввода и вывода на конкретной бирже.", ""])
    return out

def inventory(network):
    file = Path("tests/data/first_run_dex_2026-11-26") / f"raindex_{network}_protected_wallets.json.gz"
    with gzip.open(file, "rt", encoding="utf8") as stream:
        data = json.load(stream)
    paid = [r for pair in data["pairs"] for case in pair["cases"] for r in case.values()
            if isinstance(r, dict) and r.get("stage") == "paid"]
    if network == "ethereum":
        with gzip.open(file.with_name("raindex_v3_ethereum.json.gz"),"rt",encoding="utf8") as stream:
            legacy=json.load(stream)
        paid.extend(c["by_output"] for p in legacy["pairs"] for c in p["cases"] if c["by_output"]["stage"]=="paid")
    return sorted({r["book"] for r in paid}), f"Raindex · {len(paid)} выплат своего кошелька через модуль; версии и состав валют в разделе Raindex"
